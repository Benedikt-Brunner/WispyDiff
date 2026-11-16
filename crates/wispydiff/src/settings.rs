//! The persisted preferences (`prefs.rs`) as an app global, with short accessors.

use std::sync::Arc;

use gpui_kit::{App, Global};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::backend::Backend;
use crate::prefs::Prefs;

/// The backend owns the preferences store.
pub struct Settings(pub Arc<Backend>);

impl Global for Settings {}

pub fn init(backend: Arc<Backend>, cx: &mut App) {
    cx.set_global(Settings(backend));
}

fn prefs(cx: &App) -> &Prefs {
    cx.global::<Settings>().0.prefs()
}

/// One of `allowed`, or `fallback`.
pub fn pref(cx: &App, key: &str, allowed: &[&str], fallback: &str) -> String {
    prefs(cx).get_str(key, allowed, fallback)
}

pub fn set_pref(cx: &App, key: &str, value: &str) {
    prefs(cx).set_str(key, value);
}

pub fn list(cx: &App, key: &str) -> Vec<String> {
    prefs(cx).get_list(key)
}

pub fn set_list(cx: &App, key: &str, values: Vec<String>) {
    prefs(cx).set_list(key, &values);
}

pub fn record(cx: &App, key: &str) -> Map<String, Value> {
    prefs(cx).get_record(key)
}

pub fn set_record(cx: &App, key: &str, value: Map<String, Value>) {
    prefs(cx).set(key, value);
}

pub fn get<T: DeserializeOwned>(cx: &App, key: &str) -> Option<T> {
    prefs(cx).get(key)
}

pub fn set<T: Serialize>(cx: &App, key: &str, value: T) {
    prefs(cx).set(key, value);
}

pub fn recent_prs(cx: &App) -> Vec<String> {
    prefs(cx).recent_prs()
}

pub fn remember_pr(cx: &App, label: &str) {
    prefs(cx).remember_pr(label);
}
