/// Domain matching rules for deciding proxy behavior
#[derive(Debug, Clone)]
pub struct DomainRules {
    proxy: Vec<DomainPattern>,
    direct: Vec<DomainPattern>,
    block: Vec<DomainPattern>,
}

/// A domain pattern with matching type
#[derive(Debug, Clone)]
pub struct DomainPattern {
    pub pattern: String,
    pub match_type: MatchType,
}

/// How to match a domain
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchType {
    /// Exact string match
    Exact,
    /// Suffix match: pattern "github.com" matches "api.github.com"
    Suffix,
    /// Prefix match: pattern "api" matches "api.github.com"
    Prefix,
}

/// Action to take for a domain
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DomainAction {
    /// Route through proxy with MITM
    Proxy,
    /// Connect directly without proxy
    Direct,
    /// Block the connection
    Block,
}

impl DomainRules {
    /// Get the default GitHub proxy rules
    pub fn github_defaults() -> Self {
        Self {
            proxy: vec![
                // GitHub main site
                DomainPattern::suffix("github.com"),
                DomainPattern::suffix("github.io"),
                // GitHub API
                DomainPattern::exact("api.github.com"),
                // GitHub user content
                DomainPattern::suffix("githubusercontent.com"),
                DomainPattern::suffix("githubassets.com"),
                // GitHub development
                DomainPattern::suffix("github.dev"),
                DomainPattern::exact("codespaces.githubusercontent.com"),
                // GitHub CDN and media
                DomainPattern::suffix("objects.githubusercontent.com"),
                DomainPattern::exact("media.githubusercontent.com"),
                DomainPattern::exact("camo.githubusercontent.com"),
                DomainPattern::exact("avatars.githubusercontent.com"),
                DomainPattern::exact("cloud.githubusercontent.com"),
                // GitHub raw content
                DomainPattern::exact("raw.githubusercontent.com"),
                // GitHub upload
                DomainPattern::exact("uploads.github.com"),
                // GitHub status
                DomainPattern::exact("status.github.com"),
                // GitHub education
                DomainPattern::exact("education.github.com"),
                // GitHub desktop
                DomainPattern::exact("desktop.githubusercontent.com"),

                // Steam domains
                DomainPattern::suffix("steampowered.com"),
                DomainPattern::suffix("steamstatic.com"),
                DomainPattern::exact("steamserver.net"),
            ],
            direct: vec![],
            block: vec![],
        }
    }

    /// Classify a domain into an action
    pub fn classify(&self, domain: &str) -> DomainAction {
        let domain_lower = domain.to_lowercase();

        // Check block list first
        for pattern in &self.block {
            if pattern.matches(&domain_lower) {
                return DomainAction::Block;
            }
        }

        // Check proxy list
        for pattern in &self.proxy {
            if pattern.matches(&domain_lower) {
                return DomainAction::Proxy;
            }
        }

        // Check direct list
        for pattern in &self.direct {
            if pattern.matches(&domain_lower) {
                return DomainAction::Direct;
            }
        }

        // Default: direct
        DomainAction::Direct
    }

    /// Add a domain to the proxy list
    pub fn add_proxy(&mut self, pattern: DomainPattern) {
        self.proxy.push(pattern);
    }

    /// Add a domain to the direct list
    pub fn add_direct(&mut self, pattern: DomainPattern) {
        self.direct.push(pattern);
    }
}

impl DomainPattern {
    /// Create an exact match pattern
    pub fn exact(domain: &str) -> Self {
        Self {
            pattern: domain.to_lowercase(),
            match_type: MatchType::Exact,
        }
    }

    /// Create a suffix match pattern (matches domain and all subdomains)
    pub fn suffix(domain: &str) -> Self {
        Self {
            pattern: domain.to_lowercase(),
            match_type: MatchType::Suffix,
        }
    }

    /// Create a prefix match pattern
    pub fn prefix(pattern: &str) -> Self {
        Self {
            pattern: pattern.to_lowercase(),
            match_type: MatchType::Prefix,
        }
    }

    /// Check if this pattern matches the given domain.
    /// The domain must already be lowercased by the caller.
    pub fn matches(&self, domain: &str) -> bool {
        match self.match_type {
            MatchType::Exact => domain == self.pattern,
            MatchType::Suffix => {
                // "github.com" matches "github.com" and "*.github.com"
                domain == self.pattern || domain.ends_with(&format!(".{}", self.pattern))
            }
            MatchType::Prefix => domain.starts_with(&self.pattern),
        }
    }
}

