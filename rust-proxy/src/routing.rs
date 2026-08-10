use crate::config::{ConfigEntry, OpenAiResponsesMode, RoutingVisibility, UpstreamType};

/// The result of resolving a route for an incoming request.
#[derive(Debug, Clone)]
pub struct RouteResult {
    pub channel_name: String,
    pub upstream_type: UpstreamType,
    pub target_url: String,
    pub system_prompt: Option<String>,
    pub auth_header: Option<String>,
    pub auth_value: Option<String>,
    pub claude_code_compat: bool,
    pub resolved_model: Option<String>,
    pub virtual_model: Option<String>,
    pub return_real_model: bool,
    pub responses_mode: Option<OpenAiResponsesMode>,
}

/// Paths that can be model-routed (API endpoints).
fn is_model_routed_path(pathname: &str) -> bool {
    pathname.starts_with("/v1/")
        || pathname.starts_with("/openai/v1/")
        || pathname.starts_with("/anthropic/v1/")
}

/// Infer expected provider type from pathname prefix.
fn infer_expected_provider_type(pathname: &str) -> Option<UpstreamType> {
    if pathname.starts_with("/openai/") {
        Some(UpstreamType::OpenAI)
    } else if pathname.starts_with("/anthropic/") {
        Some(UpstreamType::Anthropic)
    } else if pathname == "/v1/messages" {
        // Anthropic Messages API also uses /v1/ prefix
        Some(UpstreamType::Anthropic)
    } else if pathname.starts_with("/v1/") {
        Some(UpstreamType::OpenAI)
    } else {
        None
    }
}

/// Parse type-forced prefix. Returns (stripped_path, forced_type) if the path
/// starts with `/openai/` or `/anthropic/`.
pub fn parse_type_forced_prefix(pathname: &str) -> (String, Option<UpstreamType>) {
    #[allow(clippy::bind_instead_of_map)]
    if let Some(rest) = pathname.strip_prefix("/openai/") {
        (format!("/{rest}"), Some(UpstreamType::OpenAI))
    } else if let Some(rest) = pathname.strip_prefix("/anthropic/") {
        (format!("/{rest}"), Some(UpstreamType::Anthropic))
    } else {
        (pathname.to_string(), None)
    }
}

/// Strip a leading `/openai/` or `/anthropic/` type-forced prefix from `path`,
/// returning a slice that begins at the following `/`. Returns the original
/// slice untouched when no such prefix is present. Used to normalize paths at
/// the URL-building boundary so callers that forward the raw request pathname
/// (e.g. failover fallbacks) cannot double the prefix onto targetBaseUrl.
fn strip_type_forced_prefix(path: &str) -> &str {
    if let Some(rest) = path.strip_prefix("/openai") {
        rest
    } else if let Some(rest) = path.strip_prefix("/anthropic") {
        rest
    } else {
        path
    }
}

/// Try to match an explicit `/providers/{channelName}/...` path.
pub fn resolve_explicit_route(
    pathname: &str,
    search: &str,
    providers: &std::collections::HashMap<String, ConfigEntry>,
) -> Option<RouteResult> {
    let parsed = parse_explicit_route_path(pathname)?;
    let entry = providers.get(&parsed.channel_name)?;
    if !entry.enabled {
        return None;
    }
    Some(build_route_result(&parsed.channel_name, entry, &parsed.remaining_path, search))
}

struct ParsedExplicitRoute {
    channel_name: String,
    remaining_path: String,
}

fn parse_explicit_route_path(pathname: &str) -> Option<ParsedExplicitRoute> {
    let rest = pathname.strip_prefix("/providers/")?;
    let slash_pos = rest.find('/')?;
    let channel_name = rest[..slash_pos].to_string();
    let remaining_path = rest[slash_pos..].to_string();
    Some(ParsedExplicitRoute { channel_name, remaining_path })
}

/// Resolve routes by model name. Returns all matching routes sorted by priority (desc).
pub fn resolve_routes_by_model(
    pathname: &str,
    search: &str,
    model: &str,
    forced_type: Option<UpstreamType>,
    providers: &std::collections::HashMap<String, ConfigEntry>,
    aliases: &std::collections::HashMap<String, crate::config::AliasTarget>,
) -> Vec<RouteResult> {
    if model.is_empty() || !is_model_routed_path(pathname) {
        return vec![];
    }

    let expected_type = forced_type.or_else(|| infer_expected_provider_type(pathname));
    let expected_type = match expected_type {
        Some(t) => t,
        None => return vec![],
    };

    // Check aliases first
    if let Some(alias_target) = aliases.get(model) {
        let targets = alias_target.targets.as_deref().unwrap_or(&[]);
        let primary = if targets.is_empty() {
            vec![crate::config::ModelAliasTarget {
                provider: alias_target.provider.clone(),
                model: alias_target.model.clone(),
            }]
        } else {
            targets.to_vec()
        };

        let mut results: Vec<RouteResult> = primary
            .iter()
            .filter_map(|target| {
                resolve_explicit_target_route(
                    pathname,
                    search,
                    target,
                    expected_type.clone(),
                    providers,
                )
                .map(|mut route| {
                    route.virtual_model = Some(model.to_string());
                    route.return_real_model = alias_target.return_real_model;
                    route.resolved_model = Some(target.model.clone());
                    route
                })
            })
            .collect();

        deduplicate_route_results(&mut results);
        return results;
    }

    // Search providers by model
    find_routes_by_model(model, expected_type, pathname, search, providers)
}

fn resolve_explicit_target_route(
    pathname: &str,
    search: &str,
    target: &crate::config::ModelAliasTarget,
    expected_type: UpstreamType,
    providers: &std::collections::HashMap<String, ConfigEntry>,
) -> Option<RouteResult> {
    let entry = resolve_provider_by_ref(&target.provider, providers)?;

    if entry.routing_visibility.as_ref() == Some(&RoutingVisibility::ExplicitOnly) {
        return None;
    }

    let upstream_type = entry.upstream_type.clone();
    if upstream_type != expected_type {
        return None;
    }

    let channel_name = entry
        .provider_uuid
        .as_ref()
        .and_then(|uuid| {
            providers
                .iter()
                .find(|(_, e)| e.provider_uuid.as_deref() == Some(uuid.as_str()))
                .map(|(name, _)| name.clone())
        })
        .unwrap_or_else(|| target.provider.clone());

    Some(build_route_result(&channel_name, entry, pathname, search))
}

fn resolve_provider_by_ref<'a>(
    reference: &str,
    providers: &'a std::collections::HashMap<String, ConfigEntry>,
) -> Option<&'a ConfigEntry> {
    // Try direct name match first
    if let Some(entry) = providers.get(reference) {
        return Some(entry);
    }
    // Try UUID match
    providers.values().find(|e| e.provider_uuid.as_deref() == Some(reference))
}

pub fn build_route_result(
    channel_name: &str,
    entry: &ConfigEntry,
    path: &str,
    search: &str,
) -> RouteResult {
    // Normalize any type-forced prefix (`/openai/` or `/anthropic/`) off the path before
    // joining it onto the provider's targetBaseUrl. The initial request handler already
    // strips this prefix, but failover/fallback callers forward the raw request pathname;
    // without stripping here, an Anthropic provider whose targetBaseUrl already ends with
    // `/anthropic` (e.g. https://open.bigmodel.cn/api/anthropic) would be forwarded to
    // `.../anthropic/anthropic/v1/messages` and 404 upstream.
    let path = strip_type_forced_prefix(path);

    // TS behavior: for OpenAI type, strip /v1 prefix so the provider's
    // targetBaseUrl (which includes /v1) doesn't double up.
    // For Anthropic, pass path through unchanged.
    let normalized_path =
        if entry.upstream_type == UpstreamType::OpenAI && is_model_routed_path(path) {
            &path[3..] // strip leading "/v1"
        } else {
            path
        };

    let target_url = if search.is_empty() {
        format!("{}{}", entry.target_base_url, normalized_path)
    } else {
        format!("{}{}?{}", entry.target_base_url, normalized_path, search)
    };

    RouteResult {
        channel_name: channel_name.to_string(),
        upstream_type: entry.upstream_type.clone(),
        target_url,
        system_prompt: entry.system_prompt.clone(),
        auth_header: entry.auth.as_ref().map(|a| match a.header {
            crate::config::RouteAuthHeader::XApiKey => "x-api-key".to_string(),
            crate::config::RouteAuthHeader::Authorization => "authorization".to_string(),
        }),
        auth_value: entry.auth.as_ref().map(|a| a.value.clone()),
        claude_code_compat: entry.claude_code_compat,
        resolved_model: None,
        virtual_model: None,
        return_real_model: false,
        responses_mode: entry.responses_mode.clone(),
    }
}

fn find_routes_by_model(
    model: &str,
    expected_type: UpstreamType,
    pathname: &str,
    search: &str,
    providers: &std::collections::HashMap<String, ConfigEntry>,
) -> Vec<RouteResult> {
    let mut candidates: Vec<(&String, &ConfigEntry)> = providers
        .iter()
        .filter(|(_, e)| {
            e.enabled
                && e.upstream_type == expected_type
                && e.routing_visibility.as_ref() != Some(&RoutingVisibility::ExplicitOnly)
        })
        .collect();

    candidates.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then_with(|| a.0.cmp(b.0)));

    candidates
        .iter()
        .filter(|(_, entry)| {
            entry.models.as_ref().is_some_and(|models| models.iter().any(|m| m.model == model))
        })
        .map(|(name, entry)| {
            let mut route = build_route_result(name, entry, pathname, search);
            route.resolved_model = Some(model.to_string());
            route
        })
        .collect()
}

fn deduplicate_route_results(results: &mut Vec<RouteResult>) {
    let mut seen = std::collections::HashSet::new();
    results.retain(|r| seen.insert(r.channel_name.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigEntry, ModelConfig};

    fn anthropic_entry(base: &str, model: &str) -> ConfigEntry {
        ConfigEntry {
            upstream_type: UpstreamType::Anthropic,
            target_base_url: base.to_string(),
            system_prompt: None,
            auth: None,
            models: Some(vec![ModelConfig {
                model: model.to_string(),
                context: None,
                extra: serde_json::Map::new(),
            }]),
            priority: 0,
            enabled: true,
            routing_visibility: None,
            responses_mode: None,
            extra_fields: None,
            provider_uuid: None,
            auto_sync_models: false,
            claude_code_compat: false,
        }
    }

    fn openai_entry(base: &str, model: &str) -> ConfigEntry {
        ConfigEntry {
            upstream_type: UpstreamType::OpenAI,
            target_base_url: base.to_string(),
            system_prompt: None,
            auth: None,
            models: Some(vec![ModelConfig {
                model: model.to_string(),
                context: None,
                extra: serde_json::Map::new(),
            }]),
            priority: 0,
            enabled: true,
            routing_visibility: None,
            responses_mode: None,
            extra_fields: None,
            provider_uuid: None,
            auto_sync_models: false,
            claude_code_compat: false,
        }
    }

    /// Regression: `/anthropic/` type-forced prefix must not be doubled onto an
    /// Anthropic provider whose targetBaseUrl already ends with `/anthropic`.
    /// This previously produced `.../anthropic/anthropic/v1/messages` and a 404.
    #[test]
    fn anthropic_type_forced_prefix_not_doubled() {
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "bigmodel-linhai-a".to_string(),
            anthropic_entry("https://open.bigmodel.cn/api/anthropic", "glm-5.2"),
        );
        let aliases = std::collections::HashMap::new();

        // (a) initial route: caller passes the already-stripped path
        let (stripped, forced) = parse_type_forced_prefix("/anthropic/v1/messages");
        let routes = resolve_routes_by_model(
            &stripped,
            "beta=true",
            "glm-5.2",
            forced,
            &providers,
            &aliases,
        );
        assert_eq!(
            routes[0].target_url,
            "https://open.bigmodel.cn/api/anthropic/v1/messages?beta=true"
        );

        // (b) failover/fallback path: caller forwards the RAW prefixed pathname —
        // build_route_result must still strip the prefix.
        let routes_raw = resolve_routes_by_model(
            "/anthropic/v1/messages",
            "beta=true",
            "glm-5.2",
            None,
            &providers,
            &aliases,
        );
        assert_eq!(
            routes_raw[0].target_url,
            "https://open.bigmodel.cn/api/anthropic/v1/messages?beta=true"
        );
    }

    /// Regression: `/openai/` prefix must yield a correct OpenAI URL whether the
    /// caller passes the stripped path or the raw prefixed pathname.
    #[test]
    fn openai_type_forced_prefix_not_doubled() {
        let mut providers = std::collections::HashMap::new();
        providers.insert("oai".to_string(), openai_entry("https://api.openai.com/v1", "gpt-4o"));
        let aliases = std::collections::HashMap::new();

        let (stripped, forced) = parse_type_forced_prefix("/openai/v1/chat/completions");
        let routes = resolve_routes_by_model(&stripped, "", "gpt-4o", forced, &providers, &aliases);
        assert_eq!(routes[0].target_url, "https://api.openai.com/v1/chat/completions");

        let routes_raw = resolve_routes_by_model(
            "/openai/v1/chat/completions",
            "",
            "gpt-4o",
            None,
            &providers,
            &aliases,
        );
        assert_eq!(routes_raw[0].target_url, "https://api.openai.com/v1/chat/completions");
    }

    /// A bare `/v1/messages` (no type-forced prefix) is unaffected.
    #[test]
    fn bare_v1_path_unchanged() {
        let mut providers = std::collections::HashMap::new();
        providers.insert(
            "anthropic-direct".to_string(),
            anthropic_entry("https://api.anthropic.com", "claude-3"),
        );
        let aliases = std::collections::HashMap::new();
        let routes =
            resolve_routes_by_model("/v1/messages", "", "claude-3", None, &providers, &aliases);
        assert_eq!(routes[0].target_url, "https://api.anthropic.com/v1/messages");
    }
}
