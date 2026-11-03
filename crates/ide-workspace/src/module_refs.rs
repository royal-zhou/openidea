//! JavaScript / TypeScript module resolution for Go to Declaration.
//!
//! Go to Declaration asks the language server first; this module is the fallback
//! for the module specifiers (`@/components/Foo.vue`, `@acme/ui`, `#internal`)
//! that tsserver/Volar cannot resolve when the alias lives in `vite.config.ts`,
//! `vue.config.js` or a pnpm workspace instead of `tsconfig.json` `paths`.
//!
//! Sources, in the order they are tried (the first hit wins):
//!
//!   1. relative paths (`./`, `../`, `/`) with extension / `index` probing;
//!   2. `tsconfig.json` / `jsconfig.json`: `paths` (exact, then longest `*`
//!      prefix) and `baseUrl`, following `extends`;
//!   3. `vite.config.*`, `vitest.config.*`, `vue.config.js`, `webpack.config.*`:
//!      `resolve.alias` object / array entries and `alias.set(…)` calls;
//!   4. framework defaults (Vue CLI `@` → `src`, Nuxt `~`/`@`, SvelteKit `$lib`);
//!   5. workspace packages from `pnpm-workspace.yaml` (or `package.json`
//!      `workspaces`), through their `exports` / `module` / `main`, then the
//!      matching file under `src/` when that entry is a build artifact;
//!   6. `package.json` `imports` (`#internal`).
//!
//! A language server still answers first and follows `types` into `dist/`.
//! [`prefer_workspace_source`] rewrites that location to the workspace package's
//! source, following barrel re-exports to the file that defines the symbol.
//!
//! When several files match (`Button.tsx` and `Button.vue`) every candidate is
//! returned; the UI shows its chooser.
//!
//! No project JavaScript is executed: configs are parsed as text (tree-sitter
//! for the TypeScript/JavaScript ones), and dynamic expressions are ignored —
//! Settings › Languages › TypeScript › Path aliases is the explicit fallback.

use ide_core::{IpcPath, Location, Pos, Range, jsonc};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Extensions tried for an extension-less specifier, most specific first.
const EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte", "json"];

/// Config files that may carry aliases, nearest-first order is by directory.
const JS_CONFIGS: &[&str] = &[
    "vite.config.ts",
    "vite.config.mts",
    "vite.config.cts",
    "vite.config.js",
    "vite.config.mjs",
    "vite.config.cjs",
    "vitest.config.ts",
    "vitest.config.mts",
    "vitest.config.js",
    "vitest.config.mjs",
    "vue.config.js",
    "vue.config.ts",
    "webpack.config.js",
    "webpack.config.ts",
    "webpack.config.mjs",
    "webpack.config.cjs",
];

/// One resolved file (or folder) and the rule that produced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModuleTarget {
    pub path: IpcPath,
    pub dir: bool,
    /// Human-readable origin, e.g. `tsconfig paths "@/*"` or `vite alias "@"`.
    pub via: String,
}

impl ModuleTarget {
    fn new(path: PathBuf, dir: bool, via: impl Into<String>) -> Self {
        Self { path: path.into(), dir, via: via.into() }
    }
}

/// Resolves `specifier` imported from `from` in the project at `root`.
pub fn resolve(root: &Path, from: &Path, specifier: &str) -> Vec<ModuleTarget> {
    resolve_with(root, from, specifier, &[])
}

/// [`resolve`] plus the `prefix=target` aliases from Settings › Languages › TypeScript,
/// the fallback for aliases that live in dynamic JS configs.
pub fn resolve_with(root: &Path, from: &Path, specifier: &str, settings_aliases: &[String]) -> Vec<ModuleTarget> {
    let Some(spec) = clean(specifier) else { return Vec::new() };
    let mut out = Vec::new();
    let from_dir = from.parent().unwrap_or(root);

    if spec.starts_with('.') || spec.starts_with('/') {
        let base = if spec.starts_with('/') {
            normalize(&root.join(spec.trim_start_matches('/')))
        } else {
            normalize(&from_dir.join(&spec))
        };
        add(&mut out, probe(&base), "relative path");
        return out;
    }

    if spec.starts_with('#') {
        if add(&mut out, package_imports(from, root, &spec), "package.json imports") {
            return out;
        }
        return out;
    }

    if let Some(config) = nearest_tsconfig(from, root) {
        let ts = load_tsconfig(&config, root);
        let (targets, via) = ts.resolve(&spec);
        if add(&mut out, targets, via) {
            return out;
        }
    }

    for config in js_configs(from, root) {
        let Some(text) = fs::read_to_string(&config).ok() else { continue };
        let label = config.file_name().and_then(|n| n.to_str()).unwrap_or("alias");
        let dir = config.parent().unwrap_or(root).to_path_buf();
        for (prefix, target) in parse_aliases(&text, &dir, root) {
            if let Some(rest) = strip_prefix_literal(&spec, &prefix) {
                let base = normalize(&target.join(rest));
                if add(&mut out, probe(&base), format!("{label} alias \"{prefix}\"")) {
                    return out;
                }
            }
        }
    }

    if let Some((targets, via)) = settings_alias(root, &spec, settings_aliases) {
        if add(&mut out, targets, via) {
            return out;
        }
    }

    if let Some((targets, via)) = framework_alias(root, &spec) {
        if add(&mut out, targets, via) {
            return out;
        }
    }

    if let Some(targets) = workspace_package(from, &spec) {
        if add(&mut out, targets, "workspace package") {
            return out;
        }
    }

    if let Some(targets) = node_modules_package(from, &spec) {
        if add(&mut out, targets, "node_modules package") {
            return out;
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Specifiers and file probing

/// Trims a specifier, drops `?query` / `#fragment`, percent-decodes it, and
/// rejects URLs / data: / node: builtins.
fn clean(specifier: &str) -> Option<String> {
    let s = specifier.trim();
    if s.is_empty() || s.starts_with("//") || s.contains("://") || s.starts_with("data:") || s.starts_with("node:") {
        return None;
    }
    let s = s.split('?').next().unwrap_or_default();
    // A leading `#` is a package import, not a fragment.
    let s = if s.starts_with('#') { s } else { s.split('#').next().unwrap_or_default() };
    let decoded = percent_decode(s);
    (!decoded.is_empty()).then_some(decoded)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// Removes `.` and resolves `..` lexically (no filesystem access).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn with_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Existing files `base` may name: the file itself, `base.<ext>`, `base/index.<ext>`,
/// and a directory's package entry. Ordered `.ts`/`.tsx` first, `.d.ts` last.
fn probe(base: &Path) -> Vec<PathBuf> {
    let base = normalize(base);
    let mut out: Vec<PathBuf> = Vec::new();
    if base.is_file() {
        out.push(base);
        return dedupe(out);
    }
    if base.is_dir() {
        for ext in EXTS {
            let p = base.join(format!("index.{ext}"));
            if p.is_file() {
                out.push(p);
            }
        }
        if let Some(p) = package_entry_file(&base) {
            out.push(p);
        }
        return dedupe(out);
    }
    for ext in EXTS {
        let p = with_ext(&base, ext);
        if p.is_file() {
            out.push(p);
        }
    }
    let declarations = with_ext(&base, "d.ts");
    if declarations.is_file() {
        out.push(declarations);
    }
    dedupe(out)
}

fn dedupe(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

fn add(out: &mut Vec<ModuleTarget>, paths: Vec<PathBuf>, via: impl Into<String>) -> bool {
    let via = via.into();
    let mut added = false;
    for path in paths {
        let dir = path.is_dir();
        let target = ModuleTarget::new(path, dir, via.clone());
        if !out.iter().any(|t| t.path == target.path) {
            out.push(target);
            added = true;
        }
    }
    added
}

/// Splits `spec` at a literal alias prefix. `@` only matches `@/…`, so
/// `@scope/pkg` does not match a Vue CLI `@` alias.
fn strip_prefix_literal<'a>(spec: &'a str, prefix: &str) -> Option<&'a str> {
    if spec == prefix {
        return Some("");
    }
    let rest = spec.strip_prefix(prefix)?;
    if rest.starts_with('/') {
        return Some(rest.trim_start_matches('/'));
    }
    // `@` (Vue CLI) and `~` (Nuxt) are folder aliases: `@/x`, never `@scope`.
    if prefix.ends_with('/') { Some(rest) } else { None }
}

// ---------------------------------------------------------------------------
// tsconfig / jsconfig

#[derive(Default, Clone)]
struct TsConfig {
    base_url: Option<PathBuf>,
    paths: Vec<(String, Vec<String>)>,
    paths_dir: PathBuf,
}

impl TsConfig {
    fn resolve(&self, spec: &str) -> (Vec<PathBuf>, String) {
        if let Some((pattern, replacement)) = self.match_paths(spec) {
            let mut out = Vec::new();
            for value in replacement {
                let value = value.replace('*', &self.wildcard_capture(spec, &pattern));
                out.extend(probe(&self.paths_dir.join(value)));
            }
            if !out.is_empty() {
                return (out, format!("tsconfig paths \"{pattern}\""));
            }
        }
        if let Some(base) = &self.base_url {
            let out = probe(&normalize(&base.join(spec)));
            if !out.is_empty() {
                return (out, "tsconfig baseUrl".to_owned());
            }
        }
        (Vec::new(), String::new())
    }

    fn wildcard_capture(&self, spec: &str, pattern: &str) -> String {
        match pattern.find('*') {
            Some(star) => spec
                .strip_prefix(&pattern[..star])
                .and_then(|s| s.strip_suffix(&pattern[star + 1..]))
                .unwrap_or_default()
                .to_owned(),
            None => String::new(),
        }
    }

    fn match_paths(&self, spec: &str) -> Option<(String, Vec<String>)> {
        if let Some((key, values)) = self.paths.iter().find(|(key, _)| key == spec) {
            return (!values.is_empty()).then(|| (key.clone(), values.clone()));
        }
        let mut best: Option<(&str, &Vec<String>)> = None;
        for (key, values) in &self.paths {
            let Some(star) = key.find('*') else { continue };
            let (pre, post) = (&key[..star], &key[star + 1..]);
            if spec.len() < pre.len() + post.len() || !spec.starts_with(pre) || !spec.ends_with(post) {
                continue;
            }
            if best.is_none_or(|(b, _)| pre.len() > b.len()) {
                best = Some((key, values));
            }
        }
        best.map(|(key, values)| (key.to_owned(), values.clone()))
    }
}

/// The nearest config above `from`, stopping at the project root.
fn nearest_tsconfig(from: &Path, root: &Path) -> Option<PathBuf> {
    let mut dir = from.parent()?;
    loop {
        for name in ["tsconfig.json", "jsconfig.json"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if dir == root {
            return None;
        }
        dir = dir.parent()?;
    }
}

fn load_tsconfig(path: &Path, root: &Path) -> TsConfig {
    let mut seen = HashSet::new();
    load_tsconfig_inner(path, root, &mut seen)
}

fn load_tsconfig_inner(path: &Path, root: &Path, seen: &mut HashSet<PathBuf>) -> TsConfig {
    let dir = path.parent().unwrap_or(root).to_path_buf();
    let mut out = TsConfig { paths_dir: dir.clone(), ..TsConfig::default() };
    if !seen.insert(path.to_path_buf()) {
        return out;
    }
    let Ok(text) = fs::read_to_string(path) else { return out };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&jsonc::strip(&text)) else { return out };

    if let Some(extends) = value.get("extends") {
        for spec in strings(extends) {
            if let Some(base) = resolve_extends(&spec, &dir, root) {
                let inherited = load_tsconfig_inner(&base, root, seen);
                if out.base_url.is_none() {
                    out.base_url = inherited.base_url;
                }
                if out.paths.is_empty() {
                    out.paths = inherited.paths;
                    out.paths_dir = inherited.paths_dir;
                }
            }
        }
    }

    if let Some(co) = value.get("compilerOptions") {
        if let Some(base) = co.get("baseUrl").and_then(serde_json::Value::as_str) {
            out.base_url = Some(normalize(&dir.join(base)));
        }
        if let Some(paths) = co.get("paths").and_then(serde_json::Value::as_object) {
            out.paths = paths.iter().map(|(k, v)| (k.clone(), strings(v))).collect();
            out.paths_dir = out.base_url.clone().unwrap_or_else(|| dir.clone());
        } else if out.base_url.is_some() && !out.paths.is_empty() {
            // Inherited `paths` follow this config's `baseUrl` (TypeScript merges them).
            out.paths_dir = out.base_url.clone().unwrap();
        }
    }
    out
}

fn resolve_extends(spec: &str, dir: &Path, root: &Path) -> Option<PathBuf> {
    let direct =
        if spec.starts_with('.') || Path::new(spec).is_absolute() { Some(normalize(&dir.join(spec))) } else { None };
    if let Some(candidate) = direct {
        for p in [candidate.clone(), with_ext(&candidate, "json"), candidate.join("tsconfig.json")] {
            if p.is_file() {
                return Some(p);
            }
        }
        return None;
    }
    // Package: walk up looking for node_modules/<spec>.
    for ancestor in dir.ancestors() {
        if !ancestor.starts_with(root) {
            break;
        }
        let package = ancestor.join("node_modules").join(spec);
        if package.is_dir() {
            let config = package.join("tsconfig.json");
            if config.is_file() {
                return Some(config);
            }
            if let Ok(text) = fs::read_to_string(package.join("package.json"))
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(field) = value.get("tsconfig").and_then(serde_json::Value::as_str)
            {
                let config = package.join(field);
                if config.is_file() {
                    return Some(config);
                }
            }
        }
    }
    None
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(items) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// JS config aliases (vite / vitest / vue / webpack)

fn js_configs(from: &Path, root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dir = from.parent();
    while let Some(d) = dir {
        for name in JS_CONFIGS {
            let p = d.join(name);
            if p.is_file() {
                found.push(p);
            }
        }
        if found.iter().any(|p| p.parent() == Some(d)) {
            return found;
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    found
}

/// `alias` entries of a JS config: object form, array form, and `alias.set(…)`.
fn parse_aliases(text: &str, dir: &Path, root: &Path) -> Vec<(String, PathBuf)> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&tree_sitter_typescript::LANGUAGE_TSX.into()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else { return Vec::new() };
    let mut out = Vec::new();
    walk(tree.root_node(), &mut |node: tree_sitter::Node| match node.kind() {
        "pair" => {
            if key_of(node, text).as_deref() == Some("alias")
                && let Some(value) = node.child_by_field_name("value")
            {
                parse_alias_value(value, text, dir, root, &mut out);
            }
        }
        "call_expression" => {
            let Some(function) = node.child_by_field_name("function") else { return };
            let callee = function.utf8_text(text.as_bytes()).unwrap_or_default();
            if !callee.ends_with(".alias.set") {
                return;
            }
            let Some(arguments) = node.child_by_field_name("arguments") else { return };
            let mut cursor = arguments.walk();
            let args: Vec<_> = arguments.named_children(&mut cursor).collect();
            let Some(key) = args.first().and_then(|n| unquote(n.utf8_text(text.as_bytes()).ok()?)) else { return };
            let Some(value) = args.get(1).and_then(|n| eval_expression(n.utf8_text(text.as_bytes()).ok()?, dir, root))
            else {
                return;
            };
            out.push((key, value));
        }
        _ => {}
    });
    out
}

fn parse_alias_value(node: tree_sitter::Node, text: &str, dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    match node.kind() {
        "object" => {
            let mut cursor = node.walk();
            for pair in node.named_children(&mut cursor).filter(|n| n.kind() == "pair") {
                let Some(key) = key_of(pair, text) else { continue };
                let Some(value) = pair.child_by_field_name("value") else { continue };
                let Some(path) = eval_expression(value.utf8_text(text.as_bytes()).unwrap_or_default(), dir, root)
                else {
                    continue;
                };
                out.push((key, path));
            }
        }
        "array" => {
            let mut cursor = node.walk();
            for entry in node.named_children(&mut cursor).filter(|n| n.kind() == "object") {
                let mut fields = HashMap::new();
                let mut inner = entry.walk();
                for pair in entry.named_children(&mut inner).filter(|n| n.kind() == "pair") {
                    if let (Some(key), Some(value)) = (key_of(pair, text), pair.child_by_field_name("value")) {
                        fields.insert(key, value.utf8_text(text.as_bytes()).unwrap_or_default().to_owned());
                    }
                }
                let Some(find) = fields.get("find").and_then(|s| unquote(s)) else { continue };
                let Some(replacement) = fields.get("replacement").and_then(|s| eval_expression(s, dir, root)) else {
                    continue;
                };
                out.push((find, replacement));
            }
        }
        _ => {}
    }
}

fn key_of(pair: tree_sitter::Node, text: &str) -> Option<String> {
    let key = pair.child_by_field_name("key")?;
    unquote(key.utf8_text(text.as_bytes()).ok()?).or_else(|| {
        let raw = key.utf8_text(text.as_bytes()).ok()?.trim();
        (!raw.is_empty() && raw.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| raw.to_owned())
    })
}

fn walk(node: tree_sitter::Node, f: &mut impl FnMut(tree_sitter::Node)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, f);
    }
}

/// Evaluates the literal subset of an alias expression; `None` for dynamic ones.
fn eval_expression(expr: &str, dir: &Path, root: &Path) -> Option<PathBuf> {
    let expr = expr.trim().trim_end_matches(',').trim();
    if let Some(literal) = unquote(expr) {
        return Some(join_alias(&literal, dir, root));
    }
    if let Some(url) = between(expr, "new URL(", ")")
        && let Some(path) = first_string(url)
    {
        return Some(normalize(&dir.join(path)));
    }
    for call in ["path.resolve(", "resolve(", "path.join(", "join("] {
        if let Some(inner) = between(expr, call, ")")
            && let Some(path) = first_string(inner)
        {
            let base = if inner.contains("__dirname") || inner.contains("import.meta") { dir } else { root };
            return Some(normalize(&base.join(path)));
        }
    }
    None
}

fn join_alias(value: &str, dir: &Path, root: &Path) -> PathBuf {
    if value.starts_with('/') {
        normalize(&root.join(value.trim_start_matches('/')))
    } else if value.starts_with('.') {
        normalize(&dir.join(value))
    } else {
        normalize(&dir.join(value))
    }
}

fn first_string(text: &str) -> Option<&str> {
    let start = text.find(['\'', '"', '`'])?;
    let quote = text[start..].chars().next()?;
    let rest = &text[start + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let rest = &text[from..];
    let to = rest.rfind(end).unwrap_or(rest.len());
    Some(&rest[..to])
}

fn unquote(text: &str) -> Option<String> {
    let s = text.trim();
    let quote = s.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') || s.len() < 2 || !s.ends_with(quote) {
        return None;
    }
    Some(s[quote.len_utf8()..s.len() - quote.len_utf8()].to_owned())
}

// ---------------------------------------------------------------------------
// Framework defaults

/// User-configured `prefix=target` aliases (Settings › Languages › TypeScript).
fn settings_alias(root: &Path, spec: &str, aliases: &[String]) -> Option<(Vec<PathBuf>, String)> {
    for alias in aliases {
        let Some((prefix, target)) = alias.split_once('=') else { continue };
        let (prefix, target) = (prefix.trim(), target.trim());
        if prefix.is_empty() || target.is_empty() {
            continue;
        }
        let Some(rest) = strip_prefix_literal(spec, prefix) else { continue };
        let base = if Path::new(target).is_absolute() { PathBuf::from(target) } else { root.join(target) };
        let targets = probe(&normalize(&base.join(rest)));
        if !targets.is_empty() {
            return Some((targets, format!("settings alias \"{prefix}\"")));
        }
    }
    None
}

fn framework_alias(root: &Path, spec: &str) -> Option<(Vec<PathBuf>, &'static str)> {
    let has = |name: &str| root.join(name).is_file();
    let nuxt = has("nuxt.config.ts") || has("nuxt.config.js") || has("nuxt.config.mjs");
    let vue_cli = has("vue.config.js") || has("vue.config.ts");
    let svelte = has("svelte.config.js") || has("svelte.config.ts");

    if nuxt {
        for (prefix, dir) in [("~~", root), ("@@", root), ("~", root), ("@", root)] {
            if let Some(rest) = strip_prefix_literal(spec, prefix) {
                let targets = probe(&normalize(&dir.join(rest)));
                if !targets.is_empty() {
                    return Some((targets, "Nuxt alias"));
                }
            }
        }
    }
    if svelte && let Some(rest) = strip_prefix_literal(spec, "$lib") {
        let targets = probe(&normalize(&root.join("src/lib").join(rest)));
        if !targets.is_empty() {
            return Some((targets, "SvelteKit $lib"));
        }
    }
    if vue_cli && let Some(rest) = strip_prefix_literal(spec, "@") {
        let targets = probe(&normalize(&root.join("src").join(rest)));
        if !targets.is_empty() {
            return Some((targets, "Vue CLI @"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// pnpm workspace

fn workspace_root(from: &Path) -> Option<PathBuf> {
    from.ancestors().find(|dir| dir.join("pnpm-workspace.yaml").is_file()).map(Path::to_path_buf)
}

fn workspace_package(from: &Path, spec: &str) -> Option<Vec<PathBuf>> {
    let workspace = workspace_root(from)?;
    let packages = workspace_packages(&workspace);
    // `@scope/name/sub` → package `@scope/name`; `name/sub` → `name`.
    let candidates: Vec<&str> = {
        let mut all = vec![spec];
        let mut s = spec;
        while let Some((head, _)) = s.rsplit_once('/') {
            all.push(head);
            s = head;
        }
        all
    };
    for candidate in candidates {
        let Some(dir) = packages.get(candidate) else { continue };
        let rest = spec.strip_prefix(candidate).unwrap_or_default().trim_start_matches('/');
        let targets = package_entry(dir, rest);
        if !targets.is_empty() {
            return Some(targets);
        }
    }
    None
}

fn workspace_packages(root: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    for pattern in workspace_patterns(root) {
        for dir in expand_glob(root, &pattern) {
            if let Some(name) = package_name(&dir) {
                map.entry(name).or_insert(dir);
            }
        }
    }
    if let Some(name) = package_name(root) {
        map.entry(name).or_insert_with(|| root.to_path_buf());
    }
    map
}

fn workspace_patterns(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("pnpm-workspace.yaml")) else { return Vec::new() };
    let mut out = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        if !in_packages {
            if trimmed.starts_with("packages:") {
                in_packages = true;
                // Inline `packages: ["apps/*"]`.
                if let Some(list) = trimmed.strip_prefix("packages:") {
                    for item in list.trim().trim_matches(['[', ']']).split(',') {
                        let item = item.trim().trim_matches(['\'', '"']);
                        if !item.is_empty() {
                            out.push(item.to_owned());
                        }
                    }
                }
            }
            continue;
        }
        if let Some(item) = trimmed.strip_prefix('-') {
            let item = item.trim().trim_matches(['\'', '"']);
            if !item.is_empty() {
                out.push(item.to_owned());
            }
            continue;
        }
        // A new top-level key ends the list.
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
    }
    out
}

fn expand_glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pattern = pattern.trim().trim_start_matches("./").trim_end_matches('/');
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    expand_segments(base, &segments, 0, &mut out);
    out
}

fn expand_segments(dir: &Path, segments: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 12 {
        return;
    }
    let Some((segment, rest)) = segments.split_first() else {
        if dir.is_dir() {
            out.push(dir.to_path_buf());
        }
        return;
    };
    match *segment {
        "**" => {
            expand_segments(dir, rest, depth + 1, out);
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && !is_skipped(&path) {
                        expand_segments(&path, segments, depth + 1, out);
                    }
                }
            }
        }
        seg if seg.contains('*') => {
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if glob_match(seg, &name) && path.is_dir() && !is_skipped(&path) {
                        expand_segments(&path, rest, depth + 1, out);
                    }
                }
            }
        }
        seg => expand_segments(&dir.join(seg), rest, depth + 1, out),
    }
}

fn is_skipped(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "node_modules" || name == ".git" || name == "dist")
}

fn glob_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        Some((pre, post)) => name.starts_with(pre) && name.ends_with(post) && name.len() >= pre.len() + post.len(),
        None => pattern == name,
    }
}

fn package_name(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("name").and_then(serde_json::Value::as_str).map(str::to_owned)
}

fn package_entry(dir: &Path, rest: &str) -> Vec<PathBuf> {
    prefer_package_source(dir, published_package_entry(dir, rest))
}

fn published_package_entry(dir: &Path, rest: &str) -> Vec<PathBuf> {
    let manifest = read_manifest(dir);
    if !rest.is_empty() {
        if let Some(exports) = manifest.as_ref().and_then(|m| m.get("exports"))
            && let Some(target) = exports_target(exports, &format!("./{rest}"))
        {
            let targets = probe(&normalize(&dir.join(target)));
            if !targets.is_empty() {
                return targets;
            }
        }
        let direct = probe(&normalize(&dir.join(rest)));
        if !direct.is_empty() {
            return direct;
        }
        let under_src = probe(&normalize(&dir.join("src").join(rest)));
        if !under_src.is_empty() {
            return under_src;
        }
        return Vec::new();
    }
    if let Some(exports) = manifest.as_ref().and_then(|m| m.get("exports"))
        && let Some(target) = exports_target(exports, ".")
    {
        let targets = probe(&normalize(&dir.join(target)));
        if !targets.is_empty() {
            return targets;
        }
    }
    for field in ["types", "module", "main"] {
        if let Some(target) = manifest.as_ref().and_then(|m| m.get(field)).and_then(serde_json::Value::as_str) {
            let targets = probe(&normalize(&dir.join(target)));
            if !targets.is_empty() {
                return targets;
            }
        }
    }
    let mut targets = probe(&dir.join("src").join("index"));
    if targets.is_empty() {
        targets = probe(&dir.join("index"));
    }
    targets
}

fn read_manifest(dir: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn exports_target(exports: &serde_json::Value, key: &str) -> Option<String> {
    let entry = match exports {
        serde_json::Value::String(s) => return (key == ".").then(|| s.clone()),
        serde_json::Value::Object(map) => map.get(key)?,
        _ => return None,
    };
    condition_target(entry)
}

fn condition_target(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(map) => {
            for condition in ["types", "import", "module", "browser", "default", "require"] {
                if let Some(target) = map.get(condition).and_then(condition_target) {
                    return Some(target);
                }
            }
            None
        }
        _ => None,
    }
}

fn package_entry_file(dir: &Path) -> Option<PathBuf> {
    let manifest = read_manifest(dir)?;
    for field in ["module", "main", "types"] {
        if let Some(target) = manifest.get(field).and_then(serde_json::Value::as_str) {
            let path = normalize(&dir.join(target));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// node_modules packages (`@tong/assistant`)

/// A bare npm package specifier resolved through `node_modules` (pnpm symlinks
/// included): `@scope/name/sub` → package dir + subpath.
fn node_modules_package(from: &Path, spec: &str) -> Option<Vec<PathBuf>> {
    let (package, sub) = split_package_spec(spec)?;
    for ancestor in from.ancestors() {
        let dir = ancestor.join("node_modules").join(&package);
        if !dir.is_dir() {
            continue;
        }
        // pnpm keeps packages in `.pnpm/<id>/node_modules/<name>`; open the real
        // path, so a workspace dependency lands in `packages/<name>`.
        let dir = if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            dunce::canonicalize(&dir).unwrap_or(dir)
        } else {
            dir
        };
        let targets = package_entry(&dir, sub);
        if !targets.is_empty() {
            return Some(targets);
        }
    }
    None
}

/// Splits `@scope/name/sub` into (`@scope/name`, `sub`) and `name/sub` into
/// (`name`, `sub`).
fn split_package_spec(spec: &str) -> Option<(String, &str)> {
    let mut parts = spec.splitn(3, '/');
    let first = parts.next()?;
    if first.is_empty() || first == "." || first == ".." {
        return None;
    }
    if let Some(name) = first.strip_prefix('@') {
        if name.is_empty() {
            return None;
        }
        let package = parts.next()?;
        if package.is_empty() {
            return None;
        }
        let sub = parts.next().unwrap_or_default();
        Some((format!("@{name}/{package}"), sub))
    } else {
        let sub = parts.next().unwrap_or_default();
        Some((first.to_owned(), sub))
    }
}

// ---------------------------------------------------------------------------
// package.json `imports` (`#internal`)

fn package_imports(from: &Path, root: &Path, spec: &str) -> Vec<PathBuf> {
    let mut dir = from.parent();
    while let Some(d) = dir {
        let manifest = d.join("package.json");
        if manifest.is_file()
            && let Some(imports) = read_manifest(d).and_then(|m| m.get("imports").cloned())
        {
            let Some(target) = imports_target(&imports, spec) else { return Vec::new() };
            return probe(&normalize(&d.join(target)));
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    Vec::new()
}

fn imports_target(imports: &serde_json::Value, spec: &str) -> Option<String> {
    let map = imports.as_object()?;
    if let Some(target) = map.get(spec).and_then(condition_target) {
        return Some(target);
    }
    for (pattern, value) in map {
        let Some(star) = pattern.find('*') else { continue };
        let (pre, post) = (&pattern[..star], &pattern[star + 1..]);
        if spec.starts_with(pre) && spec.ends_with(post) {
            let capture = &spec[pre.len()..spec.len() - post.len()];
            let target = condition_target(value)?;
            return Some(target.replace('*', capture));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Workspace source instead of dist/lib build output

/// Build directories a package publishes. `src/` next to one of these is the code to edit.
const BUILD_DIRS: &[&str] = &["dist", "build", "esm", "cjs", "out", "lib"];

const SOURCE_EXTS: &[&str] = &[".ts", ".tsx", ".vue", ".svelte", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// Rewrites a language-server location that landed in a workspace package's
/// build output (`dist/types/index.d.ts`, `dist/index.js`, …) to the defining
/// source file. Third-party packages and virtual documents are unchanged.
pub fn prefer_workspace_source(root: &Path, location: Location) -> Location {
    if location.uri.as_deref().is_some_and(ide_core::is_virtual_uri)
        || ide_core::is_virtual_uri(&location.path.to_string_lossy())
    {
        return location;
    }
    let Some(package_dir) = workspace_package_containing(root, &location.path) else {
        return location;
    };
    let Some(source) = published_to_source(&package_dir, &location.path) else {
        return location;
    };
    let symbol = fs::read_to_string(&location.path).ok().and_then(|text| identifier_at(&text, location.range));
    if let Some(name) = symbol
        && let Some(def) = find_export(&source, &name)
    {
        return rebase_location(root, def);
    }
    Location::file(rebase_under(root, &source), Range::point(Pos::new(0, 0)))
}

/// `/var` and `/private/var` are the same directory on macOS. Keep the prefix the
/// project was opened with, so the editor does not treat the source as a new root.
fn rebase_under(root: &Path, path: &Path) -> PathBuf {
    let real_root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let real_path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    real_path.strip_prefix(&real_root).map(|rel| root.join(rel)).unwrap_or(real_path)
}

fn rebase_location(root: &Path, mut location: Location) -> Location {
    location.path = rebase_under(root, &location.path).into();
    location
}

/// Workspace packages publish `dist/` but are edited in `src/`. Packages under
/// `node_modules` keep the file their manifest names.
fn prefer_package_source(dir: &Path, targets: Vec<PathBuf>) -> Vec<PathBuf> {
    if targets.is_empty() || path_has_component(dir, "node_modules") {
        return targets;
    }
    let mut out = Vec::new();
    for path in targets {
        match published_to_source(dir, &path) {
            Some(source) => out.push(source),
            None => out.push(path),
        }
    }
    dedupe(out)
}

fn workspace_package_containing(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = dunce::canonicalize(path).ok()?;
    if !path.starts_with(&root) || path_has_component(&path, "node_modules") {
        return None;
    }
    let mut dir = path.parent()?.to_path_buf();
    loop {
        if dir.join("package.json").is_file() {
            return Some(dir);
        }
        if dir == root || !dir.pop() {
            break;
        }
    }
    None
}

fn path_has_component(path: &Path, name: &str) -> bool {
    path.components().any(|c| c.as_os_str() == name)
}

/// `packages/sceneCore/dist/types/index.d.ts` → `packages/sceneCore/src/index.ts`
/// when that source file exists.
fn published_to_source(package_dir: &Path, published: &Path) -> Option<PathBuf> {
    let published_real = dunce::canonicalize(published).unwrap_or_else(|_| normalize(published));
    let package_real = dunce::canonicalize(package_dir).unwrap_or_else(|_| normalize(package_dir));
    let rel = published_real.strip_prefix(&package_real).or_else(|_| published.strip_prefix(package_dir)).ok()?;
    let mut comps = rel.components();
    let first = comps.next()?.as_os_str();
    if !BUILD_DIRS.iter().any(|dir| first == *dir) {
        return None;
    }
    let rest: PathBuf = comps.collect();
    if rest.as_os_str().is_empty() {
        return None;
    }
    for base in source_bases(&rest) {
        // Join onto the caller's package dir so `/var` does not become `/private/var`.
        let candidate = package_dir.join("src").join(base);
        if let Some(source) = best_source(probe(&candidate)) {
            return Some(source);
        }
    }
    None
}

fn source_bases(rest: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(stripped) = strip_leading(rest, &["types", "esm", "cjs"]) {
        push_unique(&mut out, &strip_code_extension(&stripped));
        if let Some(again) = strip_leading(&stripped, &["types", "esm", "cjs"]) {
            push_unique(&mut out, &strip_code_extension(&again));
        }
    }
    push_unique(&mut out, &strip_code_extension(rest));
    out
}

fn strip_leading(path: &Path, names: &[&str]) -> Option<PathBuf> {
    let mut comps = path.components().peekable();
    let mut stripped = false;
    while let Some(c) = comps.peek() {
        let name = c.as_os_str();
        if names.iter().any(|n| name == *n) {
            comps.next();
            stripped = true;
        } else {
            break;
        }
    }
    let rest: PathBuf = comps.collect();
    (stripped && !rest.as_os_str().is_empty()).then_some(rest)
}

fn strip_code_extension(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return path.to_path_buf();
    };
    let stem = name
        .strip_suffix(".d.ts")
        .or_else(|| name.strip_suffix(".d.mts"))
        .or_else(|| name.strip_suffix(".d.cts"))
        .or_else(|| name.strip_suffix(".d.tsx"))
        .or_else(|| SOURCE_EXTS.iter().find_map(|ext| name.strip_suffix(ext)));
    match stem {
        Some(stem) if !stem.is_empty() => path.with_file_name(stem),
        _ => path.to_path_buf(),
    }
}

fn push_unique(out: &mut Vec<PathBuf>, path: &Path) {
    if !out.iter().any(|p| p == path) {
        out.push(path.to_path_buf());
    }
}

fn best_source(paths: Vec<PathBuf>) -> Option<PathBuf> {
    let rank = |path: &Path| -> usize {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".d.ts") || name.ends_with(".d.mts") || name.ends_with(".d.cts") {
            return 100;
        }
        SOURCE_EXTS.iter().position(|ext| name.ends_with(ext)).unwrap_or(50)
    };
    paths.into_iter().filter(|path| rank(path) < 100).min_by_key(|path| rank(path))
}

fn find_export(file: &Path, name: &str) -> Option<Location> {
    if !is_ident(name) || is_keyword(name) {
        return None;
    }
    let mut seen = HashSet::new();
    find_export_in(file, name, 0, &mut seen)
}

fn find_export_in(file: &Path, name: &str, depth: u32, seen: &mut HashSet<PathBuf>) -> Option<Location> {
    if depth > 8 {
        return None;
    }
    let file = dunce::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    if !seen.insert(file.clone()) {
        return None;
    }
    let text = fs::read_to_string(&file).ok()?;
    let masked = mask_comments_and_strings(&text);
    if name == "default" {
        return default_export_location(&file, &text, &masked);
    }
    if let Some(at) = declaration_byte(&masked, name) {
        let (line, col) = line_col(&text, at);
        return Some(ident_location(&file, line, col, name));
    }
    if let Some((module, imported)) = reexport_of(&masked, name) {
        let next = resolve_relative(&file, &module)?;
        if imported == "*" {
            return Some(Location::file(next, Range::point(Pos::new(0, 0))));
        }
        return find_export_in(&next, &imported, depth + 1, seen)
            .or_else(|| Some(Location::file(next, Range::point(Pos::new(0, 0)))));
    }
    for module in star_reexports(&masked) {
        if let Some(next) = resolve_relative(&file, &module)
            && let Some(hit) = find_export_in(&next, name, depth + 1, seen)
        {
            return Some(hit);
        }
    }
    None
}

fn resolve_relative(from_file: &Path, spec: &str) -> Option<PathBuf> {
    let dir = from_file.parent()?;
    best_source(probe(&normalize(&dir.join(spec))))
}

fn declaration_byte(text: &str, name: &str) -> Option<usize> {
    const PREFIXES: &[&str] = &[
        "export default async function ",
        "export default function ",
        "export default class ",
        "export async function ",
        "export function ",
        "export class ",
        "export const ",
        "export let ",
        "export var ",
        "export type ",
        "export interface ",
        "export enum ",
        "async function ",
        "function ",
        "class ",
        "const ",
        "let ",
        "var ",
        "interface ",
        "enum ",
        "type ",
    ];
    let mut best: Option<(usize, usize)> = None;
    for (rank, prefix) in PREFIXES.iter().enumerate() {
        let mut from = 0;
        while let Some(rel) = text[from..].find(prefix) {
            let at = from + rel;
            let name_at = at + prefix.len();
            if boundary_before(text, at) && ident_at(text, name_at, name) && best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, name_at));
                break;
            }
            from = name_at;
        }
    }
    best.map(|(_, at)| at)
}

fn default_export_location(file: &Path, text: &str, masked: &str) -> Option<Location> {
    let at = keyword_at(masked, "export default")?;
    let (line, col) = line_col(text, at);
    Some(Location::file(file.to_path_buf(), Range::point(Pos::new(line, col))))
}

fn keyword_at(text: &str, keyword: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = text[from..].find(keyword) {
        let at = from + rel;
        if boundary_before(text, at) && boundary_after(text, at + keyword.len()) {
            return Some(at);
        }
        from = at + keyword.len();
    }
    None
}

fn reexport_of(text: &str, name: &str) -> Option<(String, String)> {
    for (_, stmt) in import_export_statements(text) {
        if let Some(hit) = binding_in_statement(&stmt, name) {
            return Some(hit);
        }
    }
    None
}

fn star_reexports(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (_, stmt) in import_export_statements(text) {
        let compact = stmt.split_whitespace().collect::<Vec<_>>().join(" ");
        if !(compact.starts_with("export * from ") || compact.starts_with("export type * from ")) {
            continue;
        }
        if let Some(spec) = module_specifier(&stmt)
            && is_relative_spec(spec)
        {
            out.push(spec.to_owned());
        }
    }
    out
}

fn binding_in_statement(stmt: &str, name: &str) -> Option<(String, String)> {
    let spec = module_specifier(stmt)?;
    if !is_relative_spec(spec) {
        return None;
    }
    let clause = stmt.split(" from ").next()?.trim().trim_end_matches(';').trim();
    let clause =
        clause.trim_start_matches("import").trim_start_matches("export").trim().trim_start_matches("type").trim();
    if let Some(imported) = named_binding(clause, name) {
        return Some((spec.to_owned(), imported));
    }
    if let Some((_, alias)) = clause.split_once("* as ") {
        let alias = alias.split([',', '{', ';']).next()?.trim();
        if alias == name {
            return Some((spec.to_owned(), "*".to_owned()));
        }
    }
    let head = clause.split([',', '{']).next()?.trim();
    if head == name && is_ident(head) {
        return Some((spec.to_owned(), "default".to_owned()));
    }
    None
}

fn named_binding(clause: &str, name: &str) -> Option<String> {
    let start = clause.find('{')?;
    let end = clause.rfind('}')?;
    if end <= start {
        return None;
    }
    for part in clause[start + 1..end].split(',') {
        let part = part.trim().strip_prefix("type ").unwrap_or(part.trim()).trim();
        if part.is_empty() {
            continue;
        }
        if let Some((orig, alias)) = part.split_once(" as ") {
            if alias.trim() == name {
                return Some(orig.trim().to_owned());
            }
        } else if part == name {
            return Some(name.to_owned());
        }
    }
    None
}

fn module_specifier(stmt: &str) -> Option<&str> {
    let from = stmt.rfind(" from ")?;
    let rest = stmt[from + 6..].trim().trim_end_matches(';').trim();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(&rest[..end])
}

fn is_relative_spec(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../")
}

fn import_export_statements(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, _)) = chars.next() {
        if is_stmt_keyword(text, i, "import") || is_stmt_keyword(text, i, "export") {
            let end = statement_end(text, i);
            out.push((i, text[i..end].to_owned()));
            while chars.peek().is_some_and(|(j, _)| *j < end) {
                chars.next();
            }
        }
    }
    out
}

fn is_stmt_keyword(text: &str, i: usize, keyword: &str) -> bool {
    text[i..].starts_with(keyword) && boundary_before(text, i) && boundary_after(text, i + keyword.len())
}

fn statement_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = start;
    let mut depth = 0i32;
    let mut quote = 0u8;
    while i < bytes.len() {
        let c = bytes[i];
        if quote != 0 {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            quote = c;
            i += 1;
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
        } else if (c == b';' || c == b'\n') && depth <= 0 {
            return i + 1;
        }
        i += 1;
    }
    text.len()
}

fn identifier_at(text: &str, range: Range) -> Option<String> {
    let line = text.split('\n').nth(range.start.line as usize)?.trim_end_matches('\r');
    let start = utf16_to_byte(line, range.start.col);
    let end = utf16_to_byte(line, range.end.col.max(range.start.col));
    let (start, end) = if start == end { expand_ident(line, start) } else { (start, end) };
    let slice = line.get(start..end)?;
    is_ident(slice).then(|| slice.to_owned())
}

fn expand_ident(line: &str, at: usize) -> (usize, usize) {
    let at = at.min(line.len());
    let bytes = line.as_bytes();
    let mut start = at;
    while start > 0 && bytes[start - 1].is_ascii() && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }
    (start, end)
}

fn utf16_to_byte(line: &str, col: u32) -> usize {
    let mut units = 0u32;
    for (i, ch) in line.char_indices() {
        if units >= col {
            return i;
        }
        units += ch.len_utf16() as u32;
    }
    line.len()
}

fn line_col(text: &str, byte: usize) -> (u32, u32) {
    let byte = byte.min(text.len());
    let mut line = 0u32;
    let mut start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if i >= byte {
            break;
        }
        if b == b'\n' {
            line += 1;
            start = i + 1;
        }
    }
    let col = text.get(start..byte).map(|s| s.encode_utf16().count() as u32).unwrap_or(0);
    (line, col)
}

fn ident_location(path: &Path, line: u32, col: u32, name: &str) -> Location {
    let end = col + name.encode_utf16().count() as u32;
    Location::file(path.to_path_buf(), Range::new(Pos::new(line, col), Pos::new(line, end)))
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if is_ident_start(c) => chars.all(is_ident_char),
        _ => false,
    }
}

fn is_keyword(name: &str) -> bool {
    matches!(
        name,
        "export"
            | "import"
            | "from"
            | "function"
            | "class"
            | "const"
            | "let"
            | "var"
            | "type"
            | "interface"
            | "enum"
            | "default"
            | "as"
            | "return"
            | "async"
    )
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn ident_at(text: &str, at: usize, name: &str) -> bool {
    text[at..].starts_with(name) && boundary_after(text, at + name.len())
}

fn boundary_before(text: &str, at: usize) -> bool {
    at == 0 || text[..at].chars().next_back().is_none_or(|c| !is_ident_char(c))
}

fn boundary_after(text: &str, at: usize) -> bool {
    text[at..].chars().next().is_none_or(|c| !is_ident_char(c))
}

/// Replaces comments with spaces, keeping byte offsets stable. String contents
/// stay intact so `from './file'` specifiers can still be read.
fn mask_comments_and_strings(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0;
    let mut quote = 0u8;
    let mut line_comment = false;
    let mut block_comment = false;
    while i < bytes.len() {
        let c = bytes[i];
        if line_comment {
            if c == b'\n' {
                line_comment = false;
                out[i] = c;
            } else {
                out[i] = b' ';
            }
            i += 1;
            continue;
        }
        if block_comment {
            if c == b'*' && bytes.get(i + 1) == Some(&b'/') {
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                block_comment = false;
            } else {
                out[i] = if c == b'\n' { b'\n' } else { b' ' };
                i += 1;
            }
            continue;
        }
        if quote != 0 {
            // Keep string contents: import specifiers live in quotes. Copying them
            // also stops `//` inside a string from looking like a comment.
            out[i] = c;
            if c == b'\\' && i + 1 < bytes.len() {
                out[i + 1] = bytes[i + 1];
                i += 2;
                continue;
            }
            if c == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            line_comment = true;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            block_comment = true;
            continue;
        }
        if c == b'\'' || c == b'"' || c == b'`' {
            quote = c;
        }
        out[i] = c;
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, contents) in files {
            let p = dir.path().join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        dir
    }

    fn paths_of(targets: &[ModuleTarget]) -> Vec<String> {
        targets.iter().map(|t| t.path.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn relative_specifiers_get_extensions_and_index_files() {
        let dir = project(&[("src/a.ts", ""), ("src/b.tsx", ""), ("src/c/index.ts", ""), ("src/d.vue", "")]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();

        assert_eq!(paths_of(&resolve(root, &from, "./a")), [root.join("src/a.ts").to_string_lossy()]);
        assert_eq!(paths_of(&resolve(root, &from, "./b")), [root.join("src/b.tsx").to_string_lossy()]);
        assert_eq!(paths_of(&resolve(root, &from, "./c")), [root.join("src/c/index.ts").to_string_lossy()]);
        assert_eq!(paths_of(&resolve(root, &from, "./d.vue")), [root.join("src/d.vue").to_string_lossy()]);
    }

    #[test]
    fn nested_relative_import_without_extension() {
        let dir = project(&[("src/businessComponents/tSceneOfficeDetail/OfficeDetailBaseInfo/index.tsx", "")]);
        let root = dir.path();
        let from = root.join("src/pages/office/OfficeDetailPage.tsx");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        // A folder `OfficeDetailBaseInfo/` without an extension in the import.
        let targets = resolve(root, &from, "../../businessComponents/tSceneOfficeDetail/OfficeDetailBaseInfo");
        assert_eq!(
            paths_of(&targets),
            [root.join("src/businessComponents/tSceneOfficeDetail/OfficeDetailBaseInfo/index.tsx").to_string_lossy()]
        );
    }

    #[test]
    fn tsconfig_paths_and_base_url() {
        let dir = project(&[
            (
                "tsconfig.json",
                r#"{
                  "compilerOptions": {
                    "baseUrl": ".",
                    "paths": { "@/*": ["src/*"], "@ui": ["src/ui/index.ts"] }
                  }
                }"#,
            ),
            ("src/views/Home.vue", ""),
            ("src/ui/index.ts", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/App.vue");
        fs::write(&from, "").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@/views/Home")),
            [root.join("src/views/Home.vue").to_string_lossy()]
        );
        assert_eq!(paths_of(&resolve(root, &from, "@ui")), [root.join("src/ui/index.ts").to_string_lossy()]);
    }

    #[test]
    fn tsconfig_extends_inherits_paths() {
        let dir = project(&[
            ("tsconfig.base.json", r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } } }"#),
            ("tsconfig.json", r#"{ "extends": "./tsconfig.base.json", "compilerOptions": { "target": "ES2022" } }"#),
            ("src/a.ts", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();

        assert_eq!(paths_of(&resolve(root, &from, "@/a")), [root.join("src/a.ts").to_string_lossy()]);
    }

    #[test]
    fn vite_aliases_object_and_array_forms() {
        let dir = project(&[
            (
                "vite.config.ts",
                r#"import { fileURLToPath, URL } from 'node:url'
                   import path from 'node:path'
                   export default {
                     resolve: {
                       alias: {
                         '@': fileURLToPath(new URL('./src', import.meta.url)),
                         '~': path.resolve(__dirname, 'lib'),
                       },
                     },
                   }"#,
            ),
            ("src/App.vue", ""),
            ("lib/util.ts", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();

        assert_eq!(paths_of(&resolve(root, &from, "@/App")), [root.join("src/App.vue").to_string_lossy()]);
        assert_eq!(paths_of(&resolve(root, &from, "~/util")), [root.join("lib/util.ts").to_string_lossy()]);
    }

    #[test]
    fn vite_alias_array_form() {
        let dir = project(&[
            (
                "vite.config.ts",
                r#"export default {
                     resolve: {
                       alias: [{ find: '@', replacement: new URL('./src', import.meta.url).pathname }],
                     },
                   }"#,
            ),
            ("src/App.vue", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();
        assert_eq!(paths_of(&resolve(root, &from, "@/App")), [root.join("src/App.vue").to_string_lossy()]);
    }

    #[test]
    fn pnpm_workspace_packages_resolve_through_package_json() {
        let dir = project(&[
            ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
            ("packages/ui/package.json", r#"{ "name": "@acme/ui", "exports": { ".": "./src/index.ts" } }"#),
            ("packages/ui/src/index.ts", ""),
            ("packages/ui/src/styles.css", ""),
        ]);
        let root = dir.path();
        fs::write(root.join("package.json"), r#"{ "name": "acme" }"#).unwrap();
        let from = root.join("apps/web/src/App.vue");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@acme/ui")),
            [root.join("packages/ui/src/index.ts").to_string_lossy()]
        );
        assert_eq!(
            paths_of(&resolve(root, &from, "@acme/ui/src/styles.css")),
            [root.join("packages/ui/src/styles.css").to_string_lossy()]
        );
    }

    #[test]
    fn workspace_package_prefers_source_over_dist() {
        let dir = project(&[
            ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
            (
                "packages/sceneCore/package.json",
                r#"{"name":"@tongban/sceneCore","types":"./dist/types/index.d.ts","module":"./dist/index.js","main":"./dist/index.js"}"#,
            ),
            ("packages/sceneCore/dist/types/index.d.ts", "export { registerConfig };\n"),
            ("packages/sceneCore/dist/index.js", "export { registerConfig };\n"),
            (
                "packages/sceneCore/src/index.ts",
                "import { registerConfig } from './core/config';\nexport { registerConfig };\n",
            ),
            ("packages/sceneCore/src/core/config.ts", "export function registerConfig() {}\n"),
        ]);
        let root = dir.path();
        fs::write(root.join("package.json"), r#"{ "name": "app" }"#).unwrap();
        let from = root.join("standard/pc/src/main.ts");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "import { registerConfig } from '@tongban/sceneCore';\n").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@tongban/sceneCore")),
            [root.join("packages/sceneCore/src/index.ts").to_string_lossy()]
        );

        let built = root.join("packages/sceneCore/dist/types/index.d.ts");
        let loc = Location::file(built, Range::new(Pos::new(0, 9), Pos::new(0, 9 + "registerConfig".len() as u32)));
        let jumped = prefer_workspace_source(root, loc);
        assert_eq!(jumped.path.to_string_lossy(), root.join("packages/sceneCore/src/core/config.ts").to_string_lossy());
        assert_eq!(jumped.range.start, Pos::new(0, "export function ".len() as u32));
    }

    #[test]
    fn workspace_subpath_and_namespace_open_source() {
        let dir = project(&[
            ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
            (
                "packages/ui/package.json",
                r#"{"name":"@acme/ui","exports":{"./hooks/useForm":"./dist/hooks/useForm.js"}}"#,
            ),
            ("packages/ui/dist/hooks/useForm.js", "export function useForm() {}\n"),
            ("packages/ui/src/hooks/useForm.ts", "export function useForm() {}\n"),
            ("packages/ui/dist/types/index.d.ts", "export { tool };\n"),
            ("packages/ui/src/index.ts", "import * as tool from './tools';\nexport { tool };\n"),
            ("packages/ui/src/tools/index.ts", "export function run() {}\n"),
        ]);
        let root = dir.path();
        fs::write(root.join("package.json"), r#"{ "name": "app" }"#).unwrap();
        let from = root.join("apps/web/src/App.vue");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@acme/ui/hooks/useForm")),
            [root.join("packages/ui/src/hooks/useForm.ts").to_string_lossy()]
        );

        let built = root.join("packages/ui/dist/types/index.d.ts");
        let loc = Location::file(built.clone(), Range::new(Pos::new(0, 9), Pos::new(0, 13)));
        let jumped = prefer_workspace_source(root, loc);
        assert_eq!(jumped.path.to_string_lossy(), root.join("packages/ui/src/tools/index.ts").to_string_lossy());

        let already = Location::file(root.join("packages/ui/src/hooks/useForm.ts"), Range::point(Pos::new(0, 16)));
        assert_eq!(prefer_workspace_source(root, already.clone()).path, already.path);
    }

    #[test]
    fn node_modules_packages_keep_dist_even_when_src_exists() {
        let dir = project(&[
            (
                "node_modules/@tong/assistant/package.json",
                r#"{"name":"@tong/assistant","types":"./dist/index.d.ts","main":"./dist/index.js"}"#,
            ),
            ("node_modules/@tong/assistant/dist/index.d.ts", "export const x = 1;\n"),
            ("node_modules/@tong/assistant/src/index.ts", "export const x = 1;\n"),
        ]);
        let root = dir.path();
        let from = root.join("src/app.ts");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@tong/assistant")),
            [root.join("node_modules/@tong/assistant/dist/index.d.ts").to_string_lossy()]
        );
        let built = root.join("node_modules/@tong/assistant/dist/index.d.ts");
        let loc = Location::file(built.clone(), Range::new(Pos::new(0, 13), Pos::new(0, 14)));
        assert_eq!(prefer_workspace_source(root, loc).path, built.as_path());
    }

    #[test]
    fn node_modules_packages_and_subpaths() {
        let dir = project(&[
            (
                "node_modules/@tong/assistant/package.json",
                r#"{"name":"@tong/assistant","types":"./dist/index.d.ts","main":"./dist/index.js"}"#,
            ),
            ("node_modules/@tong/assistant/dist/index.d.ts", ""),
            ("node_modules/@tong/assistant/dist/index.js", ""),
            ("node_modules/@tong/assistant/styles.css", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/app.ts");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        assert_eq!(
            paths_of(&resolve(root, &from, "@tong/assistant")),
            [root.join("node_modules/@tong/assistant/dist/index.d.ts").to_string_lossy()]
        );
        assert_eq!(
            paths_of(&resolve(root, &from, "@tong/assistant/styles.css")),
            [root.join("node_modules/@tong/assistant/styles.css").to_string_lossy()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn pnpm_workspace_symlinks_open_the_real_package() {
        let dir = project(&[
            ("packages/assistant/package.json", r#"{"name":"@tong/assistant","types":"./src/index.ts"}"#),
            ("packages/assistant/src/index.ts", ""),
        ]);
        let root = dir.path();
        fs::create_dir_all(root.join("node_modules/@tong")).unwrap();
        std::os::unix::fs::symlink(root.join("packages/assistant"), root.join("node_modules/@tong/assistant")).unwrap();
        let from = root.join("src/app.ts");
        fs::create_dir_all(from.parent().unwrap()).unwrap();
        fs::write(&from, "").unwrap();

        let real = dunce::canonicalize(root).unwrap().join("packages/assistant/src/index.ts");
        assert_eq!(paths_of(&resolve(root, &from, "@tong/assistant")), [real.to_string_lossy()]);
    }

    #[test]
    fn package_json_imports() {
        let dir = project(&[
            ("package.json", r##"{ "name": "app", "imports": { "#internal/*": "./src/internal/*.ts" } }"##),
            ("src/internal/config.ts", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();
        assert_eq!(
            paths_of(&resolve(root, &from, "#internal/config")),
            [root.join("src/internal/config.ts").to_string_lossy()]
        );
    }

    #[test]
    fn scoped_packages_do_not_match_the_at_alias() {
        let dir = project(&[
            (
                "vue.config.js",
                "module.exports = { configureWebpack: { resolve: { alias: { '@': require('path').resolve(__dirname, 'src') } } } }",
            ),
            ("src/a.ts", ""),
        ]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();
        assert!(resolve(root, &from, "@scope/pkg").is_empty());
        assert_eq!(paths_of(&resolve(root, &from, "@/a")), [root.join("src/a.ts").to_string_lossy()]);
    }

    #[test]
    fn settings_path_aliases_are_the_fallback() {
        let dir = project(&[("src/alias/target.ts", "")]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();

        assert!(resolve(root, &from, "@lib/target").is_empty());
        let aliases = vec!["@lib=src/alias".to_owned()];
        assert_eq!(
            paths_of(&resolve_with(root, &from, "@lib/target", &aliases)),
            [root.join("src/alias/target.ts").to_string_lossy()]
        );
    }

    #[test]
    fn queries_and_urls_are_ignored() {
        let dir = project(&[("src/raw.txt", "")]);
        let root = dir.path();
        let from = root.join("src/main.ts");
        fs::write(&from, "").unwrap();
        assert_eq!(paths_of(&resolve(root, &from, "./raw.txt?raw")), [root.join("src/raw.txt").to_string_lossy()]);
        assert!(resolve(root, &from, "https://example.com/a.js").is_empty());
        assert!(resolve(root, &from, "node:fs").is_empty());
    }
}
