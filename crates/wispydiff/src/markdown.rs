//! GitHub comment/assistant rich text. Parsing belongs to the element's keyed state;
//! callbacks and attachment availability can change without reparsing the document.
//!
//! TextView's HTML parser has no custom-node hooks. We sanitize/partition HTML once,
//! retain ordinary rich text in TextView::html, and use its Markdown custom-block hook
//! for native disclosures, rules and fenced code. No WebView or implicit URL opener.
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use gpui_kit::component::{
    text::{markdown_ast, MarkdownExtensions, MarkdownNode, TextView, TextViewStyle},
    ActiveTheme,
};
use gpui_kit::{
    div, prelude::*, px, AnyElement, App, ElementId, Entity, Image, Pixels, RenderOnce,
    SharedString, StyleRefinement, Window,
};
use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

pub use crate::logic::file_refs::CodeRef;
use crate::logic::file_refs::{
    escape_html, parse_line_list, parse_ref, resolve_line_list, safe_decode, RefPart,
};

pub type OpenUrl = Rc<dyn Fn(&str, &mut Window, &mut App)>;
pub type ImageResolver = Rc<dyn Fn(&str) -> Option<Arc<Image>>>;

#[derive(Clone)]
pub struct CodeRefs {
    pub resolve: Rc<dyn Fn(&CodeRef) -> Option<CodeRef>>,
    pub open: Rc<dyn Fn(&CodeRef, &mut Window, &mut App)>,
}

#[derive(Clone)]
pub struct RenderOptions {
    pub on_open_url: OpenUrl,
    pub refs: Option<CodeRefs>,
    /// Return a prepared image, or None while loading. The owner schedules loading
    /// and requests a new frame when ready; rendering never performs file I/O.
    pub image: Option<ImageResolver>,
    pub selectable: bool,
    pub text_size: Pixels,
    /// Code blocks use this family; inline code uses the kit theme's mono family.
    pub mono_font_family: SharedString,
    /// Inline code's text colour (None: the kit theme's look, a background box).
    pub code_fg: Option<gpui_kit::Hsla>,
    /// Behind code blocks (None: the kit theme's).
    pub code_block_bg: Option<gpui_kit::Hsla>,
}

impl RenderOptions {
    pub fn new(on_open_url: OpenUrl) -> Self {
        Self {
            on_open_url,
            refs: None,
            image: None,
            selectable: true,
            text_size: px(14.),
            mono_font_family: "monospace".into(),
            code_fg: None,
            code_block_bg: None,
        }
    }
}

pub fn html(id: impl Into<ElementId>, html: &str, opts: &RenderOptions) -> AnyElement {
    RichText {
        id: id.into(),
        source: html.to_owned(),
        format: Format::Html,
        opts: opts.clone(),
    }
    .into_any_element()
}

pub fn markdown(
    id: impl Into<ElementId>,
    text: &str,
    breaks: bool,
    opts: &RenderOptions,
) -> AnyElement {
    RichText {
        id: id.into(),
        source: text.to_owned(),
        format: Format::Markdown { breaks },
        opts: opts.clone(),
    }
    .into_any_element()
}

/// Convenience alias for assistant outcome-card bodies (GitHub comment line breaks).
pub fn markdown_to_element(
    id: impl Into<ElementId>,
    text: &str,
    opts: &RenderOptions,
) -> AnyElement {
    markdown(id, text, true, opts)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Html,
    Markdown { breaks: bool },
}

#[derive(IntoElement)]
struct RichText {
    id: ElementId,
    source: String,
    format: Format,
    opts: RenderOptions,
}

struct Renderer {
    source: String,
    format: Format,
    plan: Rc<Plan>,
    opts: RenderOptions,
    /// Disclosures the reader toggled (away from how the HTML had them: `<details open>` or not).
    toggled: HashSet<String>,
}

impl RenderOnce for RichText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}/wispy-markdown", self.id)),
            cx,
            |_, _| Renderer {
                source: self.source.clone(),
                format: self.format,
                plan: Rc::new(Plan::parse(&self.source, self.format)),
                opts: self.opts.clone(),
                toggled: HashSet::new(),
            },
        );
        state.update(cx, |state, _| {
            if state.source != self.source || state.format != self.format {
                state.plan = Rc::new(Plan::parse(&self.source, self.format));
                state.source = self.source;
                state.format = self.format;
            }
            state.opts = self.opts;
        });
        let plan = state.read(cx).plan.clone();
        let render_state = state.clone();
        // Explicit parser revision is essential: rebuilding equivalent extension
        // closures every frame must not invalidate TextView's parsed AST.
        let extensions = MarkdownExtensions::default()
            .parser_revision(1)
            .block_parser(|node, _| match node {
                markdown_ast::Node::Code(code) if code.lang.as_deref() == Some("wispy-block") => {
                    code.value
                        .parse::<usize>()
                        .ok()
                        .map(|i| MarkdownNode::new("wispy-block", i))
                }
                _ => None,
            })
            .block_renderer("wispy-block", move |node, window, cx| {
                let index = *node.data::<usize>().expect("our custom block");
                let plan = render_state.read(cx).plan.clone();
                // Large TextViews can briefly retain the old AST while their
                // replacement parses in the background (e.g. a streaming edit).
                plan.blocks
                    .get(index)
                    .map(|block| render_block(block, &render_state, window, cx))
                    .unwrap_or_else(|| div().into_any_element())
            });
        let text_size = state.read(cx).opts.text_size;
        div()
            .id(self.id)
            .w_full()
            .min_w_0()
            .text_size(text_size)
            .child(
                TextView::markdown("document", plan.source.clone())
                    .markdown_extensions(extensions)
                    .selectable(false)
                    .scrollable(false)
                    .style(text_style(&state.read(cx).opts).paragraph_gap(gpui_kit::rems(0.5))),
            )
    }
}

#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Element {
        tag: String,
        attrs: Vec<(String, String)>,
        children: Vec<Node>,
    },
}
impl Node {
    fn tag(&self) -> &str {
        match self {
            Self::Element { tag, .. } => tag,
            _ => "",
        }
    }
    fn children(&self) -> &[Node] {
        match self {
            Self::Element { children, .. } => children,
            _ => &[],
        }
    }
    fn attr(&self, key: &str) -> Option<&str> {
        match self {
            Self::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
    fn text(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Element { children, .. } => children.iter().map(Node::text).collect(),
        }
    }
    fn special(&self) -> bool {
        matches!(self.tag(), "details" | "pre" | "hr")
            || self.suggestion_box()
            || self.children().iter().any(Node::special)
    }
    fn suggestion_box(&self) -> bool {
        self.tag() == "div"
            && self
                .attr("class")
                .is_some_and(|c| c.split_whitespace().any(|c| c == "suggestion"))
    }
    fn descendant(&self, tag: &str) -> Option<&Node> {
        if self.tag() == tag {
            Some(self)
        } else {
            self.children().iter().find_map(|n| n.descendant(tag))
        }
    }
}

fn forbidden(tag: &str) -> bool {
    matches!(
        tag,
        "script"
            | "style"
            | "iframe"
            | "object"
            | "embed"
            | "form"
            | "input"
            | "link"
            | "meta"
            | "base"
    )
}
fn unsafe_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    // Also reject embedded ASCII controls, which URL implementations can strip.
    let url: String = url.chars().filter(|c| !c.is_ascii_control()).collect();
    ["javascript:", "data:", "vbscript:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}
fn sanitize(source: &str) -> Vec<Node> {
    fn visit(handle: &Handle) -> Vec<Node> {
        match &handle.data {
            NodeData::Text { contents } => vec![Node::Text(contents.borrow().to_string())],
            NodeData::Element { name, attrs, .. } => {
                let tag = name.local.to_string();
                if forbidden(&tag) {
                    return vec![];
                }
                let children = handle.children.borrow().iter().flat_map(visit).collect();
                if matches!(tag.as_str(), "html" | "body" | "head") {
                    return children;
                }
                let attrs = attrs
                    .borrow()
                    .iter()
                    .filter_map(|attr| {
                        let key = attr.name.local.to_string();
                        let value = attr.value.to_string();
                        (!key.starts_with("on")
                            && !(matches!(key.as_str(), "href" | "src") && unsafe_url(&value)))
                        .then_some((key, value))
                    })
                    .collect();
                vec![Node::Element {
                    tag,
                    attrs,
                    children,
                }]
            }
            _ => handle.children.borrow().iter().flat_map(visit).collect(),
        }
    }
    let dom = parse_document(RcDom::default(), Default::default()).one(source);
    visit(&dom.document)
}

/// Only soft breaks in Markdown text nodes change. In particular, fenced code,
/// inline code, raw HTML and paragraph boundaries are never edited by this pass.
fn with_breaks(source: &str) -> String {
    fn offsets(node: &markdown_ast::Node, source: &str, out: &mut Vec<usize>) {
        if let markdown_ast::Node::Text(text) = node {
            if let Some(pos) = &text.position {
                for (offset, byte) in source.as_bytes()[pos.start.offset..pos.end.offset]
                    .iter()
                    .enumerate()
                {
                    if *byte == b'\n' {
                        let at = pos.start.offset + offset;
                        out.push(if at > 0 && source.as_bytes()[at - 1] == b'\r' {
                            at - 1
                        } else {
                            at
                        });
                    }
                }
            }
        }
        if let Some(children) = node.children() {
            for child in children {
                offsets(child, source, out);
            }
        }
    }
    let Ok(ast) = ::markdown::to_mdast(source, &::markdown::ParseOptions::gfm()) else {
        return source.into();
    };
    let mut positions = vec![];
    offsets(&ast, source, &mut positions);
    positions.sort_unstable();
    positions.dedup();
    let mut output = String::with_capacity(source.len() + positions.len());
    let mut start = 0;
    for at in positions {
        output.push_str(&source[start..at]);
        output.push('\\');
        start = at;
    }
    output.push_str(&source[start..]);
    output
}
fn markdown_html(source: &str, breaks: bool) -> String {
    let source = if breaks {
        with_breaks(source)
    } else {
        source.into()
    };
    let mut options = ::markdown::Options::gfm();
    // HTML/protocols pass through the DOM sanitizer below, including wispy-image:.
    options.compile.allow_dangerous_html = true;
    options.compile.allow_dangerous_protocol = true;
    options.compile.gfm_tagfilter = false;
    ::markdown::to_html_with_options(&source, &options).expect("GFM without MDX always parses")
}
fn suggestion(language: &str) -> bool {
    language.strip_prefix("suggestion").is_some_and(|rest| {
        !rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

struct Plan {
    blocks: Vec<Block>,
    source: SharedString,
}
#[cfg(test)]
thread_local! { static PARSE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
enum Block {
    Rich(Rc<Fragment>),
    Code {
        source: SharedString,
        suggestion: bool,
        key: String,
    },
    Rule(String),
    Details {
        key: String,
        initially_open: bool,
        summary: Rc<Fragment>,
        body: Vec<Block>,
    },
    List {
        key: String,
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Container {
        tag: String,
        key: String,
        children: Vec<Block>,
    },
}
impl Plan {
    fn parse(source: &str, format: Format) -> Self {
        #[cfg(test)]
        PARSE_COUNT.with(|count| count.set(count.get() + 1));
        let html = match format {
            Format::Html => source.into(),
            Format::Markdown { breaks } => markdown_html(source, breaks),
        };
        let nodes = sanitize(&html);
        let blocks = partition(&nodes, "root");
        let source = (0..blocks.len())
            .map(|i| format!("```wispy-block\n{i}\n```\n\n"))
            .collect::<String>()
            .into();
        Self { blocks, source }
    }
}
fn partition(nodes: &[Node], parent: &str) -> Vec<Block> {
    fn flush(pending: &mut Vec<Node>, blocks: &mut Vec<Block>, key: String) {
        if pending.is_empty() {
            return;
        }
        if pending
            .iter()
            .any(|n| !matches!(n, Node::Text(t) if t.trim().is_empty()))
        {
            blocks.push(Block::Rich(Rc::new(Fragment::new(
                std::mem::take(pending),
                key,
            ))));
        } else {
            pending.clear();
        }
    }
    let mut blocks = vec![];
    let mut pending = vec![];
    for (index, node) in nodes.iter().enumerate() {
        if !node.special() {
            pending.push(node.clone());
            continue;
        }
        let key = format!("{parent}/{index}");
        flush(&mut pending, &mut blocks, format!("{key}/before"));
        let children = node.children();
        blocks.push(match node.tag() {
            "details" => {
                let summary = children.iter().position(|n| n.tag() == "summary");
                let summary_nodes = summary
                    .map(|i| children[i].children().to_vec())
                    .unwrap_or_else(|| vec![Node::Text("Details".into())]);
                let body: Vec<_> = children
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| Some(*i) != summary)
                    .map(|(_, n)| n.clone())
                    .collect();
                // Explicit HTML ids keep disclosure state even if siblings move.
                let key = node
                    .attr("id")
                    .map(|id| format!("details-id:{id}"))
                    .unwrap_or(key);
                Block::Details {
                    initially_open: node.attr("open").is_some(),
                    summary: Rc::new(Fragment::new(summary_nodes, format!("{key}/summary-text"))),
                    body: partition(&body, &format!("{key}/body")),
                    key,
                }
            }
            "pre" => code_block(node, key, false),
            "div" if node.suggestion_box() && node.descendant("pre").is_some() => {
                code_block(node.descendant("pre").unwrap(), key, true)
            }
            "ul" | "ol" => Block::List {
                start: (node.tag() == "ol")
                    .then(|| node.attr("start").and_then(|s| s.parse().ok()).unwrap_or(1)),
                items: children
                    .iter()
                    .filter(|n| n.tag() == "li")
                    .enumerate()
                    .map(|(i, n)| partition(n.children(), &format!("{key}/item-{i}")))
                    .collect(),
                key,
            },
            "hr" => Block::Rule(key),
            tag => Block::Container {
                tag: tag.into(),
                children: partition(children, &key),
                key,
            },
        });
    }
    flush(&mut pending, &mut blocks, format!("{parent}/tail"));
    blocks
}
fn code_block(node: &Node, key: String, force_suggestion: bool) -> Block {
    let children = node.children();
    let code = children.iter().find(|n| n.tag() == "code");
    let lang = code
        .and_then(|n| n.attr("class"))
        .unwrap_or("")
        .split_whitespace()
        .find_map(|class| class.strip_prefix("language-"))
        .unwrap_or("");
    let suggestion = force_suggestion || suggestion(lang);
    let text = node.text();
    let fence = "`".repeat(
        text.split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    let source = format!(
        "{fence}\n{}{end}{fence}",
        text,
        end = if text.ends_with('\n') { "" } else { "\n" }
    )
    .into();
    Block::Code {
        source,
        suggestion,
        key,
    }
}

// Fragments are a prepared HTML template. Dynamic slots are resolved every frame;
// HTML serialization and TextView parsing happen only if their values change.
enum Slot {
    Code {
        original: String,
        reference: Option<CodeRef>,
        list: Option<Vec<RefPart>>,
    },
    Link {
        opening: String,
        reference: Option<CodeRef>,
    },
    Image {
        original: String,
        id: String,
        alt: String,
    },
}
enum Piece {
    Static(String),
    Slot(usize),
}
struct Fragment {
    key: String,
    pieces: Vec<Piece>,
    slots: Vec<Slot>,
    cached: RefCell<Option<Rendered>>,
}
enum Value {
    Refs(Option<Vec<RefPart>>),
    Image(Option<Arc<Image>>),
}
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Refs(a), Self::Refs(b)) => a == b,
            (Self::Image(a), Self::Image(b)) => {
                a.as_ref().map(|i| i.id()) == b.as_ref().map(|i| i.id())
            }
            _ => false,
        }
    }
}
struct Rendered {
    values: Vec<Value>,
    html: SharedString,
    links: HashMap<String, CodeRef>,
}
fn reference_url(reference: &CodeRef) -> String {
    // Stable targets also avoid misrouting a click while a large TextView is
    // still painting its previous AST during an asynchronous replacement.
    format!(
        "wispy-ref:{}:{}:{}",
        URL_SAFE_NO_PAD.encode(&reference.path),
        reference.line.map(|n| n.to_string()).unwrap_or_default(),
        reference.end.map(|n| n.to_string()).unwrap_or_default()
    )
}

fn attr_escape(s: &str) -> String {
    escape_html(s).replace('\'', "&#39;")
}
fn start_tag(tag: &str, attrs: &[(String, String)]) -> String {
    let mut s = format!("<{tag}");
    for (key, value) in attrs {
        s.push_str(&format!(" {key}=\"{}\"", attr_escape(value)));
    }
    s.push('>');
    s
}
fn void(tag: &str) -> bool {
    matches!(
        tag,
        "area" | "br" | "col" | "hr" | "img" | "param" | "source" | "track" | "wbr"
    )
}
fn serialize(node: &Node) -> String {
    match node {
        Node::Text(s) => escape_html(s),
        Node::Element {
            tag,
            attrs,
            children,
        } => {
            let mut s = start_tag(tag, attrs);
            for child in children {
                s.push_str(&serialize(child));
            }
            if !void(tag) {
                s.push_str(&format!("</{tag}>"));
            }
            s
        }
    }
}
impl Fragment {
    fn new(nodes: Vec<Node>, key: String) -> Self {
        fn add(node: &Node, blocked: bool, f: &mut Fragment) {
            let Node::Element {
                tag,
                attrs,
                children,
            } = node
            else {
                f.literal(serialize(node));
                return;
            };
            if tag == "code" && !blocked {
                let text = node.text();
                f.slot(Slot::Code {
                    original: serialize(node),
                    reference: parse_ref(&text),
                    list: parse_line_list(&text),
                });
                return;
            }
            if tag == "img" {
                if let Some(id) = node
                    .attr("src")
                    .and_then(|src| src.strip_prefix("wispy-image:"))
                {
                    f.slot(Slot::Image {
                        original: serialize(node),
                        id: id.into(),
                        alt: node.attr("alt").unwrap_or("").into(),
                    });
                    return;
                }
                // Scheme-less and file:// document images must never gain filesystem access.
                if !node.attr("src").is_some_and(http_url) {
                    f.literal(escape_html(node.attr("alt").unwrap_or("")));
                    return;
                }
            }
            if tag == "a" {
                let href = node.attr("href").unwrap_or("");
                let reference = if href.starts_with("http:")
                    || href.starts_with("https:")
                    || href.starts_with("mailto:")
                {
                    None
                } else {
                    parse_ref(&safe_decode(href))
                };
                f.slot(Slot::Link {
                    opening: start_tag(tag, attrs),
                    reference,
                });
            } else {
                f.literal(start_tag(tag, attrs));
            }
            for child in children {
                add(child, blocked || tag == "pre" || tag == "a", f);
            }
            if !void(tag) {
                f.literal(format!("</{tag}>"));
            }
        }
        let mut f = Self {
            key,
            pieces: vec![],
            slots: vec![],
            cached: RefCell::new(None),
        };
        for node in &nodes {
            add(node, false, &mut f);
        }
        f
    }
    fn literal(&mut self, text: String) {
        if let Some(Piece::Static(previous)) = self.pieces.last_mut() {
            previous.push_str(&text);
        } else {
            self.pieces.push(Piece::Static(text));
        }
    }
    fn slot(&mut self, slot: Slot) {
        self.pieces.push(Piece::Slot(self.slots.len()));
        self.slots.push(slot);
    }
    fn render(&self, opts: &RenderOptions) -> SharedString {
        let resolve = |reference: &CodeRef| {
            opts.refs
                .as_ref()
                .and_then(|refs| (refs.resolve)(reference))
        };
        let values: Vec<_> = self
            .slots
            .iter()
            .map(|slot| match slot {
                Slot::Code {
                    reference, list, ..
                } => Value::Refs(if let Some(parts) = list {
                    resolve_line_list(parts, resolve)
                } else {
                    reference.as_ref().and_then(resolve).map(|reference| {
                        vec![RefPart::Reference {
                            label: String::new(),
                            reference,
                        }]
                    })
                }),
                Slot::Link { reference, .. } => {
                    Value::Refs(reference.as_ref().and_then(resolve).map(|reference| {
                        vec![RefPart::Reference {
                            label: String::new(),
                            reference,
                        }]
                    }))
                }
                Slot::Image { id, .. } => {
                    Value::Image(opts.image.as_ref().and_then(|image| image(id)))
                }
            })
            .collect();
        let mut cached = self.cached.borrow_mut();
        if let Some(previous) = cached.as_ref().filter(|previous| previous.values == values) {
            return previous.html.clone();
        }
        let mut links = HashMap::new();
        let mut link = |reference: &CodeRef| {
            let url = reference_url(reference);
            links.insert(url.clone(), reference.clone());
            format!("<a href=\"{url}\">")
        };
        let mut html = String::new();
        for piece in &self.pieces {
            let Piece::Slot(i) = piece else {
                if let Piece::Static(s) = piece {
                    html.push_str(s);
                }
                continue;
            };
            match (&self.slots[*i], &values[*i]) {
                (Slot::Code { original, list, .. }, Value::Refs(resolved)) => match resolved {
                    None => html.push_str(original),
                    Some(parts) if list.is_some() => {
                        html.push_str("<code>");
                        for part in parts {
                            match part {
                                RefPart::Text(t) => html.push_str(&escape_html(t)),
                                RefPart::Reference { label, reference } => {
                                    html.push_str(&link(reference));
                                    html.push_str(&escape_html(label));
                                    html.push_str("</a>");
                                }
                            }
                        }
                        html.push_str("</code>");
                    }
                    Some(parts) => {
                        let RefPart::Reference { reference, .. } = &parts[0] else {
                            unreachable!()
                        };
                        html.push_str(&link(reference));
                        html.push_str(original);
                        html.push_str("</a>");
                    }
                },
                (Slot::Link { opening, .. }, Value::Refs(resolved)) => {
                    if let Some(parts) = resolved {
                        let RefPart::Reference { reference, .. } = &parts[0] else {
                            unreachable!()
                        };
                        html.push_str(&link(reference));
                    } else {
                        html.push_str(opening);
                    }
                }
                (Slot::Image { original, alt, .. }, Value::Image(image)) => {
                    if let Some(image) = image {
                        // Only trusted resolver bytes may introduce an embedded image.
                        // TextView decodes/caches these natively, with no HTTP/filesystem request.
                        let uri = format!(
                            "data:{};base64,{}",
                            image.format.mime_type(),
                            STANDARD.encode(&image.bytes)
                        );
                        html.push_str(
                            &original
                                .replacen("src=\"wispy-image:", "data-wispy-image=\"", 1)
                                .replacen("<img", &format!("<img src=\"{uri}\""), 1),
                        );
                    } else {
                        html.push_str(&escape_html(alt));
                    }
                }
                _ => unreachable!(),
            }
        }
        let html: SharedString = html.into();
        *cached = Some(Rendered {
            values,
            html: html.clone(),
            links,
        });
        html
    }
}
fn http_url(url: &str) -> bool {
    let url = url.trim().to_ascii_lowercase();
    url.starts_with("http:") || url.starts_with("https:")
}
/// As the web app had them: headings at the body size (weight sets them apart). Inline code is
/// told apart by its font and a tint, with no background: gpui-kit draws inline code smaller and
/// its background box half a line too low.
fn text_style(opts: &RenderOptions) -> TextViewStyle {
    let size = opts.text_size;
    let mut style = TextViewStyle::default().heading_font_size(move |_, _| size);
    if let Some(color) = opts.code_fg {
        style = style.inline_code(gpui_kit::HighlightStyle {
            color: Some(color),
            background_color: Some(gpui_kit::transparent_black()),
            ..Default::default()
        });
    }
    if let Some(bg) = opts.code_block_bg {
        style = style.code_block(StyleRefinement::default().bg(bg).rounded(px(6.)).px(px(10.)).py(px(8.)));
    }
    style
}

fn rich(fragment: &Rc<Fragment>, state: &Entity<Renderer>, cx: &mut App) -> AnyElement {
    let opts = state.read(cx).opts.clone();
    let html = fragment.render(&opts);
    let handler_state = state.clone();
    // Entity is Send+Sync; Rc callbacks stay on the UI thread inside its state.
    let key = fragment.key.clone();
    let text = TextView::html(SharedString::from(fragment.key.clone()), html)
        .style(text_style(&opts))
        .selectable(opts.selectable)
        .scrollable(false)
        .on_link_click(move |url, _, window, cx| {
            let renderer = handler_state.read(cx);
            let opts = renderer.opts.clone();
            let reference = find_fragment(&renderer.plan.blocks, &key).and_then(|f| {
                f.cached
                    .borrow()
                    .as_ref()
                    .and_then(|cache| cache.links.get(url.as_str()).cloned())
            });
            if let (Some(reference), Some(refs)) = (reference, &opts.refs) {
                (refs.open)(&reference, window, cx);
            } else if http_url(url) {
                (opts.on_open_url)(url, window, cx);
            }
        });
    observe(
        div()
            .id(SharedString::from(format!("{}/rich", fragment.key)))
            .w_full()
            .min_w_0()
            .child(text),
    )
}
fn find_fragment<'a>(blocks: &'a [Block], key: &str) -> Option<&'a Rc<Fragment>> {
    blocks.iter().find_map(|block| match block {
        Block::Rich(f) if f.key == key => Some(f),
        Block::Details { summary, body, .. } => {
            if summary.key == key {
                Some(summary)
            } else {
                find_fragment(body, key)
            }
        }
        Block::Container { children, .. } => find_fragment(children, key),
        Block::List { items, .. } => items.iter().find_map(|item| find_fragment(item, key)),
        _ => None,
    })
}
fn render_block(
    block: &Block,
    state: &Entity<Renderer>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let opts = state.read(cx).opts.clone();
    match block {
        Block::Rich(f) => rich(f, state, cx),
        Block::Rule(key) => div()
            .id(SharedString::from(key.clone()))
            .w_full()
            .my_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .into_any_element(),
        Block::Code {
            source,
            suggestion,
            key,
        } => {
            let mut block = StyleRefinement::default().font_family(opts.mono_font_family.clone()).text_size(opts.text_size);
            if let Some(bg) = opts.code_block_bg {
                block = block.bg(bg);
            }
            let style = TextViewStyle::default().code_block(block);
            let code =
                TextView::markdown(SharedString::from(format!("{key}/code")), source.clone())
                    .selectable(opts.selectable)
                    .scrollable(false)
                    .style(style)
                    .on_link_click(|_, _, _, _| {});
            let mut box_ = div()
                .id(SharedString::from(key.clone()))
                .w_full()
                .min_w_0()
                .rounded_md()
                .overflow_hidden()
                .when_some(opts.code_block_bg, |d, bg| d.bg(bg));
            if *suggestion {
                box_ = box_.border_1().border_color(cx.theme().border).child(
                    div()
                        .px_3()
                        .py_1()
                        .bg(cx.theme().muted)
                        .child("Suggested change"),
                );
            }
            box_.child(code).into_any_element()
        }
        Block::Details { key, initially_open, summary, body } => {
            let open = *initially_open != state.read(cx).toggled.contains(key);
            let toggle_state = state.clone();
            let toggle_key = key.clone();
            let summary = div()
                .id(SharedString::from(format!("{key}/summary")))
                .role(gpui_kit::Role::Button)
                .aria_label("Toggle details")
                .aria_expanded(open)
                .flex()
                .items_start()
                .gap_1()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    toggle_state.update(cx, |state, cx| {
                        if !state.toggled.remove(&toggle_key) {
                            state.toggled.insert(toggle_key.clone());
                        }
                        cx.notify();
                    })
                })
                .child(if open { "▾" } else { "▸" })
                .child(div().flex_1().min_w_0().child(rich(summary, state, cx)));
            let mut root = div()
                .id(SharedString::from(key.clone()))
                .w_full()
                .min_w_0()
                .child(observe(summary));
            if open {
                root = root.child(observe(
                    div()
                        .id(SharedString::from(format!("{key}/body")))
                        .w_full()
                        .min_w_0()
                        .children(
                            body.iter()
                                .map(|block| render_block(block, state, window, cx)),
                        ),
                ));
            }
            root.into_any_element()
        }
        Block::Container { tag, key, children } => {
            let mut root = div()
                .id(SharedString::from(key.clone()))
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2();
            if tag == "blockquote" {
                root = root.pl_3().border_l_2().border_color(cx.theme().border);
            }
            if tag == "tr" {
                root = root.flex_row();
            }
            if tag == "td" || tag == "th" {
                root = root
                    .flex_1()
                    .p_2()
                    .border_1()
                    .border_color(cx.theme().border);
            }
            root.children(
                children
                    .iter()
                    .map(|child| render_block(child, state, window, cx)),
            )
            .into_any_element()
        }
        Block::List { key, start, items } => div()
            .id(SharedString::from(key.clone()))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .children(items.iter().enumerate().map(|(i, item)| {
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div().min_w(px(18.)).child(
                            start
                                .map(|start| format!("{}.", start.saturating_add(i as u64)))
                                .unwrap_or_else(|| "•".into()),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().flex().flex_col().gap_2().children(
                            item.iter()
                                .map(|block| render_block(block, state, window, cx)),
                        ),
                    )
            }))
            .into_any_element(),
    }
}

// Observation is free of test-only application state; production builds need not
// enable the kit's headless registration feature.
fn observe(element: gpui_kit::Stateful<gpui_kit::Div>) -> AnyElement {
    use gpui_kit::TestSupportExt;
    element.test_support().into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{point, AppContext, Context, Render, TestAppContext, VisualTestContext};
    use std::cell::Cell;

    fn options() -> RenderOptions {
        RenderOptions::new(Rc::new(|_, _, _| {}))
    }
    fn rendered(source: &str, opts: &RenderOptions) -> String {
        Fragment::new(sanitize(source), "test".into())
            .render(opts)
            .to_string()
    }
    fn resolving() -> RenderOptions {
        let mut opts = options();
        opts.refs = Some(CodeRefs {
            resolve: Rc::new(|r| {
                (r.path == "src/a.rs" || r.path == "a.rs").then(|| CodeRef {
                    path: "src/a.rs".into(),
                    ..r.clone()
                })
            }),
            open: Rc::new(|_, _, _| {}),
        });
        opts
    }

    #[test]
    fn markdown_references_all_spellings_and_refusals() {
        for (source, line, end) in [
            ("src/a.rs", None, None),
            ("./src/a.rs", None, None),
            ("src/a.rs:42", Some(42), None),
            ("src/a.rs:42-50", Some(42), Some(50)),
            ("src/a.rs#L42-L50", Some(42), Some(50)),
            ("src/a.rs#L42C2-L50", Some(42), Some(50)),
            ("src/a.rs:42:3", Some(42), None),
            ("src/a.rs:42–L50", Some(42), Some(50)),
            ("file://./src/a.rs:42", Some(42), None),
            (
                "/data/worktrees/owner/repo/abcdef0/src/a.rs:42",
                Some(42),
                None,
            ),
            (
                "file:///data/worktrees/owner/repo/abcdef0123456789/src/a.rs:42-50",
                Some(42),
                Some(50),
            ),
        ] {
            assert_eq!(
                parse_ref(source),
                Some(CodeRef {
                    path: "src/a.rs".into(),
                    line,
                    end
                }),
                "{source}"
            );
            let html = rendered(&format!("<code>{source}</code>"), &resolving());
            assert!(
                html.starts_with("<a href=\"wispy-ref:") && html.contains("\"><code>"),
                "{source}: {html}"
            );
        }
        for source in [
            "/tmp/a.rs",
            "https://example.com/a.rs",
            "a.rs:bad",
            "a.rs#L",
            "src/a.rs:42, nope",
            "name with space",
        ] {
            assert!(parse_ref(source).is_none(), "{source}");
        }
        for source in [
            "<pre><code>src/a.rs:42</code></pre>",
            "<a href=\"https://example.com\"><code>src/a.rs:42</code></a>",
            "<code>missing.rs:42</code>",
        ] {
            assert!(
                !rendered(source, &resolving()).contains("wispy-ref:"),
                "{source}"
            );
        }
    }

    #[test]
    fn markdown_line_lists_resolve_once_and_keep_individual_targets() {
        let count = Rc::new(Cell::new(0));
        let captured = count.clone();
        let mut opts = resolving();
        opts.refs.as_mut().unwrap().resolve = Rc::new(move |r| {
            captured.set(captured.get() + 1);
            Some(CodeRef {
                path: "src/a.rs".into(),
                ..r.clone()
            })
        });
        let fragment = Fragment::new(
            sanitize("<code>a.rs:42, 50-52,\t60–62</code>"),
            "test".into(),
        );
        let html = fragment.render(&opts);
        assert_eq!(count.get(), 1);
        assert!(html.contains(">a.rs:42</a>, <a"));
        assert!(html.contains(">50-52</a>,\t<a"));
        let cached = fragment.cached.borrow();
        let links = &cached.as_ref().unwrap().links;
        assert_eq!(links.len(), 3);
        assert_eq!(
            *links.values().find(|r| r.line == Some(50)).unwrap(),
            CodeRef {
                path: "src/a.rs".into(),
                line: Some(50),
                end: Some(52)
            }
        );
        drop(cached);
        assert!(!rendered("<code>missing.rs:42, 50-52</code>", &resolving()).contains("wispy-ref:"));
    }

    #[test]
    fn markdown_relative_links_decode_but_unresolved_links_stay_plain() {
        for href in ["./src/a.rs:42", "src/%61.rs#L42-L50", "file://./src/a.rs"] {
            let fragment = Fragment::new(
                sanitize(&format!("<a href=\"{href}\">read code</a>")),
                "test".into(),
            );
            let html = fragment.render(&resolving());
            assert!(html.starts_with("<a href=\"wispy-ref:"));
            assert!(html.ends_with("\">read code</a>"));
        }
        assert_eq!(
            rendered("<a href=\"missing.rs\">missing</a>", &resolving()),
            "<a href=\"missing.rs\">missing</a>"
        );
        assert_eq!(
            rendered("<a href=\"mailto:a@example.com\">mail</a>", &resolving()),
            "<a href=\"mailto:a@example.com\">mail</a>"
        );
    }

    #[test]
    fn markdown_sanitizer_removes_executable_nodes_and_attributes() {
        let source = r#"<p onclick="bad()" onmouseover="bad()">safe<script>bad()</script><style>bad</style><iframe>bad</iframe><object>bad</object><embed><form>bad<input></form><link><meta><base><a href=" JaVaScRiPt:bad()">one</a><img src="data:image/png;base64,AAAA" alt="fallback"><a href="vbscript:bad()">two</a><a href="java&#10;script:bad()">three</a></p>"#;
        let html = rendered(source, &options());
        for bad in [
            "script",
            "style",
            "iframe",
            "object",
            "embed",
            "form",
            "input",
            "link",
            "meta",
            "base",
            "onclick",
            "onmouseover",
            "href=",
            "data:",
            "bad()",
        ] {
            assert!(!html.contains(bad), "{bad}: {html}");
        }
        assert!(html.contains("safe"));
        assert!(html.contains("fallback"));
        assert_eq!(
            rendered("<img src=\"file:///etc/passwd\" alt=\"local\">", &options()),
            "local"
        );
        assert!(
            rendered("<a href=\"https://example.com\">safe</a>", &options())
                .contains("https://example.com")
        );
    }

    #[test]
    fn markdown_suggestions_pre_rules_and_gfm() {
        for lang in ["suggestion", "suggestion:-1+2", "suggestion-extra"] {
            assert!(suggestion(lang));
        }
        for lang in ["suggestions", "suggestion_x", "rust", ""] {
            assert!(!suggestion(lang));
        }
        let plan = Plan::parse(
            "```suggestion:-1+2\n<&>\n```\n\n---\n\n```rust\nlet x = 1;\n```",
            Format::Markdown { breaks: false },
        );
        assert!(
            matches!(&plan.blocks[0], Block::Code { suggestion: true, source, .. } if source.contains("<&>"))
        );
        assert!(matches!(&plan.blocks[1], Block::Rule(_)));
        assert!(matches!(
            &plan.blocks[2],
            Block::Code {
                suggestion: false,
                ..
            }
        ));
        let plan = Plan::parse(
            "<pre><code>&lt;&amp;&gt;\n```\n</code></pre><hr>",
            Format::Html,
        );
        assert!(
            matches!(&plan.blocks[0], Block::Code { source, .. } if source.starts_with("````\n<&>"))
        );
        assert!(matches!(&plan.blocks[1], Block::Rule(_)));
        let html = markdown_html(
            "| A | B |\n| - | - |\n| ~~old~~ | [link](https://example.com) |",
            false,
        );
        assert!(html.contains("<table>"));
        assert!(html.contains("<del>old</del>"));
    }

    #[test]
    fn markdown_special_blocks_keep_lists_and_suggestion_wrappers() {
        let plan = Plan::parse("<ol start=\"3\"><li>before<pre><code>x</code></pre>after</li><li><details><summary>More</summary>body</details></li></ol>", Format::Html);
        let Block::List { start, items, .. } = &plan.blocks[0] else {
            panic!("list wrapper lost")
        };
        assert_eq!(*start, Some(3));
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0][1], Block::Code { .. }));
        assert!(matches!(&items[1][0], Block::Details { .. }));
        let plan = Plan::parse("<div class=\"suggestion\"><div class=\"suggestion-head\">Suggested change</div><pre><code>x\n</code></pre></div>", Format::Html);
        assert_eq!(plan.blocks.len(), 1);
        assert!(matches!(
            &plan.blocks[0],
            Block::Code {
                suggestion: true,
                ..
            }
        ));
    }

    #[test]
    fn markdown_breaks_change_only_soft_breaks() {
        let source = "one\ntwo\n\n`code\nspan`\n\n```rust\na\nb\n```\n\n<div>raw\nhtml</div>";
        let converted = with_breaks(source);
        assert!(converted.starts_with("one\\\ntwo\n\n"), "{converted}");
        assert!(converted.contains("`code\nspan`"));
        assert!(converted.contains("```rust\na\nb\n```"));
        assert!(converted.contains("<div>raw\nhtml</div>"));
        assert!(markdown_html("one\ntwo", true).contains("one<br />\ntwo"));
        assert!(!markdown_html("one\ntwo", false).contains("<br"));
        assert!(markdown_html("one\r\ntwo", true).contains("<br />"));
        assert_eq!(with_breaks("one  \ntwo"), "one  \ntwo");
        assert!(markdown_html("- one\n  two\n- three", true).contains("one<br />"));
    }

    #[test]
    fn markdown_images_wait_for_resolver_and_cache_trusted_bytes() {
        let source = markdown_html("![image](wispy-image:123)", true);
        assert!(source.contains("wispy-image:123"), "{source}");
        let fragment = Fragment::new(sanitize(&source), "image".into());
        let mut opts = options();
        assert_eq!(fragment.render(&opts).as_str().trim(), "<p>image</p>");
        let ready = Rc::new(Cell::new(false));
        let captured = ready.clone();
        let image = Arc::new(Image::from_bytes(gpui_kit::ImageFormat::Png, vec![1, 2, 3]));
        opts.image = Some(Rc::new(move |id| {
            assert_eq!(id, "123");
            captured.get().then(|| image.clone())
        }));
        assert!(!fragment.render(&opts).contains("data:"));
        ready.set(true);
        let html = fragment.render(&opts);
        assert!(
            html.contains("src=\"data:image/png;base64,AQID\""),
            "{html}"
        );
        assert_eq!(fragment.render(&opts), html);
        ready.set(false);
        assert!(!fragment.render(&opts).contains("data:"));
    }

    struct TestView {
        source: String,
        opts: RenderOptions,
        use_markdown: bool,
    }
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(400.)).child(if self.use_markdown {
                markdown("test-markdown", &self.source, true, &self.opts)
            } else {
                html("test-markdown", &self.source, &self.opts)
            })
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
    }

    #[gpui_kit::test]
    fn markdown_details_click_toggles_body_and_retains_state(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TestView {
                source: "<details id=\"proof\"><summary>More information</summary><p>Hidden body</p><details id=\"nested\"><summary>Nested</summary><p>Nested body</p></details></details>".into(),
                opts: options(), use_markdown: false,
            });
            Root::new(view, window, cx)
        });
        let _view = view;
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(
                window.find("details-id:proof/summary").expanded(),
                Some(false)
            );
            assert!(window.try_find("details-id:proof/body").is_none());
            window.click("details-id:proof/summary", cx);
        });
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(
                window.find("details-id:proof/summary").expanded(),
                Some(true)
            );
            assert!(window.find("details-id:proof/body").visible());
            assert!(window.find("details-id:proof/body").bounds().size.height > px(0.));
            window.click("details-id:nested/summary", cx);
        });
        draw(cx);
        cx.update(|window, cx| {
            assert!(window.find("details-id:nested/body").visible());
            window.click("details-id:proof/summary", cx);
        });
        draw(cx);
        cx.update(|window, cx| {
            assert!(window.try_find("details-id:proof/body").is_none());
            window.click("details-id:proof/summary", cx);
        });
        draw(cx);
        cx.update(|window, _| assert!(window.find("details-id:nested/body").visible()));
    }

    #[gpui_kit::test]
    fn markdown_details_open_starts_expanded_and_collapses(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TestView {
                source: "<details id=\"shown\" open><summary>Shown</summary><p>Visible body</p></details>".into(),
                opts: options(), use_markdown: false,
            });
            Root::new(view, window, cx)
        });
        let _view = view;
        draw(cx);
        cx.update(|window, cx| {
            assert_eq!(window.find("details-id:shown/summary").expanded(), Some(true));
            assert!(window.find("details-id:shown/body").visible());
            window.click("details-id:shown/summary", cx);
        });
        draw(cx);
        cx.update(|window, _| assert!(window.try_find("details-id:shown/body").is_none()));
    }

    #[gpui_kit::test]
    fn markdown_link_click_calls_opener_and_never_navigates(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let urls = Rc::new(RefCell::new(Vec::new()));
        let captured = urls.clone();
        let mut opts = options();
        opts.on_open_url = Rc::new(move |url, _, _| captured.borrow_mut().push(url.to_owned()));
        let (_, cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|_| TestView {
                source: "[Open browser](https://example.com/review)".into(),
                opts,
                use_markdown: true,
            });
            Root::new(view, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| window.click_at("root/tail/rich", point(px(25.), px(8.)), cx));
        draw(cx);
        assert_eq!(&*urls.borrow(), &["https://example.com/review"]);
        assert_eq!(cx.opened_url(), None);
    }

    #[gpui_kit::test]
    fn markdown_refs_become_clickable_without_reparsing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let before = PARSE_COUNT.with(Cell::get);
        let ready = Rc::new(Cell::new(false));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let ready_callback = ready.clone();
        let open_callback = opened.clone();
        let mut opts = options();
        opts.refs = Some(CodeRefs {
            resolve: Rc::new(move |r| ready_callback.get().then(|| r.clone())),
            open: Rc::new(move |r, _, _| open_callback.borrow_mut().push(r.clone())),
        });
        let mut content = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| TestView {
                source: "[file](src/a.rs:42-50)".into(),
                opts,
                use_markdown: true,
            });
            content = Some(view.clone());
            Root::new(view, window, cx)
        });
        draw(cx);
        cx.update(|window, cx| window.click_at("root/tail/rich", point(px(10.), px(8.)), cx));
        assert!(opened.borrow().is_empty());
        ready.set(true);
        content.unwrap().update(cx, |_, cx| cx.notify());
        draw(cx);
        cx.update(|window, cx| window.click_at("root/tail/rich", point(px(10.), px(8.)), cx));
        draw(cx);
        assert_eq!(
            &*opened.borrow(),
            &[CodeRef {
                path: "src/a.rs".into(),
                line: Some(42),
                end: Some(50)
            }]
        );
        assert_eq!(PARSE_COUNT.with(Cell::get), before + 1);
        assert_eq!(cx.opened_url(), None);
    }
}
