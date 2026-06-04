use std::borrow::Cow;
use std::collections::HashMap;

use crate::core::config::{InterceptAction, InterceptRule};

/// Lightweight path pattern matcher replacing regex for simple URL patterns.
///
/// Supported pattern forms (derived from actual config usage):
/// - `"^/[^/]+/[^/]+/releases/download/.*"` → `SegmentPrefix` (segment-aware prefix)
/// - `"^/xxx/.*"` or `"/xxx/.*"` → `StartsWith("/xxx/")`
/// - `"^/.*"` or `".*"`          → `MatchAll`
/// - `"/recaptcha/.*"`           → `Contains("/recaptcha/")`
/// - `"/exact/path"`             → `Exact("/exact/path")`
#[derive(Debug, Clone)]
enum SimplePattern {
    /// Path must start with the given prefix (literal string).
    StartsWith(String),
    /// Path must exactly equal the given string.
    Exact(String),
    /// Matches any path (including empty).
    MatchAll,
    /// Path must contain the given substring.
    Contains(String),
    /// Segment-aware prefix: the pattern is split into alternating literal
    /// chunks and `[^/]+` wildcards. At match time, each wildcard consumes
    /// one or more non-`/` characters from the path, and the final literal
    /// chunk only needs to be a prefix of the remaining path (to handle `.*`).
    SegmentPrefix(Vec<SegChunk>),
}

/// One chunk of a `SegmentPrefix` pattern.
#[derive(Debug, Clone)]
enum SegChunk {
    /// Matches one or more non-`/` characters (the `[^/]+` regex fragment).
    Wild,
    /// Literal text that must match exactly (or as a prefix for the last chunk).
    Lit(String),
}

impl SimplePattern {
    /// Convert a regex-style pattern string into a `SimplePattern`.
    fn from_pattern(pattern: &str) -> Self {
        // Check for leading anchor before stripping it
        let anchored = pattern.starts_with('^');
        let p = pattern.trim_start_matches('^');

        // If the pattern contains `[^/]+`, use segment-aware matching
        if p.contains("[^/]+") {
            return Self::parse_segment_prefix(p);
        }

        // Strip trailing wildcard
        if p.ends_with(".*") {
            let prefix = &p[..p.len() - 2];
            if prefix.is_empty() {
                SimplePattern::MatchAll
            } else if anchored {
                SimplePattern::StartsWith(prefix.to_string())
            } else {
                // Pattern like "/recaptcha/.*" without leading ^ —
                // the intent is "path contains /recaptcha/"
                SimplePattern::Contains(prefix.to_string())
            }
        } else if p == ".*" || p.is_empty() {
            SimplePattern::MatchAll
        } else {
            SimplePattern::Exact(p.to_string())
        }
    }

    /// Parse a pattern containing `[^/]+` into a `SegmentPrefix`.
    ///
    /// Example: `"/[^/]+/[^/]+/releases/download/.*"` becomes
    /// `[Lit("/"), Wild, Lit("/"), Wild, Lit("/releases/download/")]`
    fn parse_segment_prefix(p: &str) -> Self {
        let mut chunks = Vec::new();
        let mut rest = p;

        // Strip trailing .*
        let trailing_wild = rest.ends_with(".*");
        if trailing_wild {
            rest = &rest[..rest.len() - 2];
        }

        // Split on `[^/]+` boundaries
        while let Some(pos) = rest.find("[^/]+") {
            if pos > 0 {
                chunks.push(SegChunk::Lit(rest[..pos].to_string()));
            }
            chunks.push(SegChunk::Wild);
            rest = &rest[pos + 5..]; // skip "[^/]+"
        }
        if !rest.is_empty() {
            chunks.push(SegChunk::Lit(rest.to_string()));
        }

        SimplePattern::SegmentPrefix(chunks)
    }

    #[inline]
    fn matches(&self, path: &str) -> bool {
        match self {
            SimplePattern::StartsWith(prefix) => path.starts_with(prefix),
            SimplePattern::Exact(s) => path == s,
            SimplePattern::MatchAll => true,
            SimplePattern::Contains(s) => path.contains(s),
            SimplePattern::SegmentPrefix(chunks) => Self::matches_segment_prefix(chunks, path),
        }
    }

    /// Match a path against a segment-prefix pattern.
    ///
    /// Walks the chunks against the path. Each `Wild` chunk consumes one or
    /// more non-`/` characters. Each `Lit` chunk must match the path at the
    /// current position. The final `Lit` chunk only needs to be a prefix of
    /// the remaining path (to support trailing `.*` semantics).
    fn matches_segment_prefix(chunks: &[SegChunk], path: &str) -> bool {
        let mut pos = 0;
        let bytes = path.as_bytes();
        let last = chunks.len().saturating_sub(1);

        for (i, chunk) in chunks.iter().enumerate() {
            match chunk {
                SegChunk::Wild => {
                    // Consume one or more non-'/' characters
                    if pos >= bytes.len() || bytes[pos] == b'/' {
                        return false; // wildcard must match at least one char
                    }
                    while pos < bytes.len() && bytes[pos] != b'/' {
                        pos += 1;
                    }
                }
                SegChunk::Lit(lit) => {
                    let lit_bytes = lit.as_bytes();
                    if i == last {
                        // Last chunk: prefix match (handles trailing .*)
                        if pos + lit_bytes.len() > bytes.len() {
                            return false;
                        }
                        return bytes[pos..].starts_with(lit_bytes);
                    } else {
                        // Non-last chunk: exact match at current position
                        if pos + lit_bytes.len() > bytes.len() {
                            return false;
                        }
                        if &bytes[pos..pos + lit_bytes.len()] != lit_bytes {
                            return false;
                        }
                        pos += lit_bytes.len();
                    }
                }
            }
        }

        // If we get here, all chunks matched and the path is fully consumed
        // (or the last chunk was a Wild, which is fine with trailing content)
        true
    }
}

/// 请求拦截匹配器
pub struct InterceptMatcher {
    exact_rules: HashMap<String, Vec<ExactRule>>,
    suffix_rules: Vec<SuffixRule>,
    wildcard_rules: Vec<WildcardRule>,
}

#[derive(Debug, Clone)]
struct ExactRule {
    path_pattern: Option<SimplePattern>,
    action: InterceptAction,
}

struct SuffixRule {
    suffix: String,
    dot_suffix: String,
    path_pattern: Option<SimplePattern>,
    action: InterceptAction,
}

struct WildcardRule {
    pattern: String,
    star_pos: usize,
    path_pattern: Option<SimplePattern>,
    action: InterceptAction,
}

impl InterceptMatcher {
    /// 从规则列表创建匹配器
    pub fn new(rules: Vec<InterceptRule>) -> Self {
        let mut exact_rules: HashMap<String, Vec<ExactRule>> = HashMap::new();
        let mut suffix_rules = Vec::new();
        let mut wildcard_rules = Vec::new();

        for mut rule in rules {
            let path_pattern = rule.from_pattern.as_ref().map(|p| SimplePattern::from_pattern(p));

            rule.from = rule.from.to_lowercase();

            if rule.from.starts_with("*.") {
                let suffix = rule.from[2..].to_lowercase();
                suffix_rules.push(SuffixRule {
                    dot_suffix: format!(".{}", suffix),
                    suffix,
                    path_pattern,
                    action: rule.action,
                });
            } else if rule.from.contains('*') {
                let star_pos = rule.from.find('*').expect("wildcard pattern contains '*' guaranteed by contains check");
                wildcard_rules.push(WildcardRule {
                    pattern: rule.from,
                    star_pos,
                    path_pattern,
                    action: rule.action,
                });
            } else {
                exact_rules
                    .entry(rule.from)
                    .or_insert_with(Vec::new)
                    .push(ExactRule {
                        path_pattern,
                        action: rule.action,
                    });
            }
        }

        Self {
            exact_rules,
            suffix_rules,
            wildcard_rules,
        }
    }

    /// 匹配域名和路径，返回对应的拦截动作
    pub fn match_domain(&self, domain: &str, path: &str) -> Option<&InterceptAction> {
        // 使用 Cow 避免热路径上的堆分配：域名已全小写时零分配
        let domain_lower: Cow<str> = if domain.as_bytes().iter().all(|b| b.is_ascii_lowercase()) {
            Cow::Borrowed(domain)
        } else {
            Cow::Owned(domain.to_ascii_lowercase())
        };

        if let Some(rules) = self.exact_rules.get(&*domain_lower) {
            for rule in rules {
                if Self::path_matches(&rule.path_pattern, path) {
                    return Some(&rule.action);
                }
            }
        }

        for rule in &self.suffix_rules {
            if *domain_lower == rule.suffix || domain_lower.ends_with(&rule.dot_suffix) {
                if Self::path_matches(&rule.path_pattern, path) {
                    return Some(&rule.action);
                }
            }
        }

        for rule in &self.wildcard_rules {
            if self.wildcard_matches(rule, &*domain_lower) {
                if Self::path_matches(&rule.path_pattern, path) {
                    return Some(&rule.action);
                }
            }
        }

        None
    }

    #[inline]
    fn wildcard_matches(&self, rule: &WildcardRule, domain: &str) -> bool {
        let prefix = &rule.pattern[..rule.star_pos];
        let suffix = &rule.pattern[rule.star_pos + 1..];
        domain.starts_with(prefix) && domain.ends_with(suffix) && domain.len() >= prefix.len() + suffix.len()
    }

    #[inline]
    fn path_matches(pattern: &Option<SimplePattern>, path: &str) -> bool {
        match pattern {
            Some(p) => p.matches(path),
            None => true,
        }
    }
}

