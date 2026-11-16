//! `WISPY_SCRIPT`: replays keystrokes and clicks inside the app (debugging and visual checks where
//! the display server won't take injected input). Steps are whitespace-separated: a keystroke in
//! GPUI's syntax (`j`, `ctrl-k`, `shift-s`, `enter`), `type:text` (`_` types a space),
//! `click:x,y` / `dblclick:x,y` / `move:x,y` / `drag:x1,y1,x2,y2` (window pixels), `sleep:ms`.

use std::time::Duration;

use gpui_kit::{point, px, App, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, Window};

pub fn start(script: String, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            for step in script.split_whitespace() {
                eprintln!("[script] {step}");
                if let Some(ms) = step.strip_prefix("sleep:") {
                    cx.background_executor().timer(Duration::from_millis(ms.parse().unwrap_or(100))).await;
                    continue;
                }
                let step = step.to_string();
                let _ = cx.update(|window, cx| run_step(&step, window, cx));
                cx.background_executor().timer(Duration::from_millis(120)).await;
            }
            eprintln!("[script] done");
        })
        .detach();
}

fn at(spec: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    let (x, y) = spec.split_once(',').unwrap_or(("0", "0"));
    point(px(x.parse().unwrap_or(0.)), px(y.parse().unwrap_or(0.)))
}

fn run_step(step: &str, window: &mut Window, cx: &mut App) {
    if let Some(text) = step.strip_prefix("type:") {
        for ch in text.chars() {
            let key = match ch {
                '_' => "space".to_string(),
                c if c.is_ascii_uppercase() => format!("shift-{}", c.to_ascii_lowercase()),
                c => c.to_string(),
            };
            if let Ok(mut k) = Keystroke::parse(&key) {
                k.key_char = Some(if ch == '_' { " ".into() } else { ch.to_string() });
                window.dispatch_keystroke(k, cx);
            }
        }
        return;
    }
    let click = |position, count, window: &mut Window, cx: &mut App| {
        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position, pressed_button: None, modifiers: Modifiers::default() }), cx);
        for n in 1..=count {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: n, first_mouse: false }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position, modifiers: Modifiers::default(), click_count: n }),
                cx,
            );
        }
    };
    if let Some(spec) = step.strip_prefix("click:") {
        click(at(spec), 1, window, cx);
    } else if let Some(spec) = step.strip_prefix("dblclick:") {
        click(at(spec), 2, window, cx);
    } else if let Some(spec) = step.strip_prefix("drag:") {
        // drag:x1,y1,x2,y2 — press, move in steps, release.
        let n: Vec<f32> = spec.split(',').filter_map(|v| v.parse().ok()).collect();
        if let [x1, y1, x2, y2] = n[..] {
            let (a, b) = (point(px(x1), px(y1)), point(px(x2), px(y2)));
            let none = Modifiers::default();
            window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: a, pressed_button: None, modifiers: none }), cx);
            window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent { button: MouseButton::Left, position: a, modifiers: none, click_count: 1, first_mouse: false }), cx);
            for i in 1..=8 {
                let t = i as f32 / 8.;
                let at = point(px(x1 + (x2 - x1) * t), px(y1 + (y2 - y1) * t));
                window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at, pressed_button: Some(MouseButton::Left), modifiers: none }), cx);
            }
            window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent { button: MouseButton::Left, position: b, modifiers: none, click_count: 1 }), cx);
        }
    } else if let Some(spec) = step.strip_prefix("move:") {
        window.dispatch_event(PlatformInput::MouseMove(MouseMoveEvent { position: at(spec), pressed_button: None, modifiers: Modifiers::default() }), cx);
    } else if let Ok(k) = Keystroke::parse(step) {
        window.dispatch_keystroke(k, cx);
    }
}
