//! Images pasted into comments. They're kept locally with the drafts (so writing works offline)
//! and referenced as `![image](wispy-image:<id>)`; submitting uploads each one to GitHub once
//! and puts its URL in their place.

use std::collections::HashMap;

use base64::Engine;

use crate::cache::Cache;
use crate::error::{Error, Result};
use crate::github::GitHubClient;
use crate::pr_ref::PrRef;
use crate::review::Payload;

/// How a draft's body refers to a local image.
pub const SCHEME: &str = "wispy-image:";

/// GitHub takes images up to 10 MB.
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Image {
    /// Checks a pasted image (type, size) and gives it a fresh id.
    pub fn new(content_type: &str, data: Vec<u8>) -> Result<Self> {
        if extension(content_type).is_none() {
            return Err(Error::Image(format!("{content_type} images can't be attached (PNG, JPEG, GIF or WebP only)")));
        }
        if data.is_empty() {
            return Err(Error::Image("the image is empty".into()));
        }
        if data.len() > MAX_BYTES {
            return Err(Error::Image(format!("the image is {} MB; GitHub takes at most 10 MB", data.len().div_ceil(1024 * 1024))));
        }
        Ok(Image { id: crate::drafts::new_id(), content_type: content_type.to_string(), data })
    }

    /// The Markdown that shows it in a comment.
    pub fn markdown(&self) -> String {
        format!("![image]({SCHEME}{})", self.id)
    }

    /// A `data:` URL, for showing it in the app.
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.content_type, base64::engine::general_purpose::STANDARD.encode(&self.data))
    }

    fn file_name(&self) -> String {
        format!("image.{}", extension(&self.content_type).unwrap_or("png"))
    }
}

fn extension(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// Ids of the local images a body refers to, in order, without repeats.
pub fn references(body: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (at, _) in body.match_indices(SCHEME) {
        let rest = &body[at + SCHEME.len()..];
        let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn replace(body: &str, urls: &HashMap<String, String>) -> String {
    let mut body = body.to_string();
    for (id, url) in urls {
        body = body.replace(&format!("{SCHEME}{id}"), url);
    }
    body
}

/// Uploads the images the payload's bodies refer to and puts their GitHub URLs in. Each image is
/// uploaded once per repo (remembered), so a submit that failed after uploading doesn't upload
/// again. Fails before anything is posted if an image is missing or GitHub refuses one.
pub async fn upload(github: &GitHubClient, cache: &Cache, pr: &PrRef, payload: &mut Payload) -> Result<()> {
    let bodies = payload
        .review_comments
        .iter_mut()
        .map(|(_, c)| &mut c.body)
        .chain(payload.summary.iter_mut().map(|(_, text)| text))
        .chain(payload.file_comments.iter_mut().map(|(_, _, body)| body))
        .chain(payload.replies.iter_mut().map(|(_, _, body)| body));
    let mut bodies: Vec<&mut String> = bodies.collect();
    let ids: Vec<String> = bodies.iter().flat_map(|b| references(b)).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let repo = format!("{}/{}", pr.owner, pr.repo);
    let mut urls = HashMap::new();
    let mut repository_id = None;
    for id in ids {
        if urls.contains_key(&id) {
            continue;
        }
        let url = match cache.image_url(&id, &repo)? {
            Some(url) => url,
            None => {
                let image = cache.image(&id)?.ok_or_else(|| Error::Image(format!("a pasted image ({SCHEME}{id}) is missing")))?;
                let repository = match repository_id {
                    Some(repository) => repository,
                    None => *repository_id.insert(github.repository_id(&pr.owner, &pr.repo).await?),
                };
                let url = github.upload_image(repository, &image.file_name(), &image.content_type, image.data).await?;
                cache.put_image_url(&id, &repo, &url)?;
                url
            }
        };
        urls.insert(id, url);
    }
    for body in &mut bodies {
        **body = replace(body, &urls);
    }
    Ok(())
}
