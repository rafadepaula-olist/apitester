use std::collections::HashMap;

use chrono::Utc;
use rand::distributions::Alphanumeric;
use rand::Rng;
use uuid::Uuid;

use crate::models::{KeyValue, Variable};

/// Variable resolution scopes ordered from lowest to highest precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableScope {
    Global,
    Collection,
    Environment,
    Data,
    Local,
}

/// Holds variables across all 5 Postman-compatible scopes.
///
/// Precedence (highest to lowest): `Local > Data > Environment > Collection > Global`.
#[derive(Debug, Clone, Default)]
pub struct VariableContext {
    pub global: HashMap<String, String>,
    pub collection: HashMap<String, String>,
    pub environment: HashMap<String, String>,
    pub data: HashMap<String, String>,
    pub local: HashMap<String, String>,
}

impl VariableContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, scope: VariableScope, key: impl Into<String>, value: impl Into<String>) {
        self.scope_map_mut(scope).insert(key.into(), value.into());
    }

    pub fn unset(&mut self, scope: VariableScope, key: &str) -> Option<String> {
        self.scope_map_mut(scope).remove(key)
    }

    /// Loads only `enabled == true` variables into the given scope.
    pub fn load_variables(&mut self, scope: VariableScope, variables: &[Variable]) {
        let map = self.scope_map_mut(scope);
        for var in variables {
            if var.enabled && !var.key.is_empty() {
                map.insert(var.key.clone(), var.value.clone());
            }
        }
    }

    /// Looks up a static variable by precedence: Local > Data > Environment > Collection > Global.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.local
            .get(key)
            .or_else(|| self.data.get(key))
            .or_else(|| self.environment.get(key))
            .or_else(|| self.collection.get(key))
            .or_else(|| self.global.get(key))
            .map(String::as_str)
    }

    fn scope_map_mut(&mut self, scope: VariableScope) -> &mut HashMap<String, String> {
        match scope {
            VariableScope::Global => &mut self.global,
            VariableScope::Collection => &mut self.collection,
            VariableScope::Environment => &mut self.environment,
            VariableScope::Data => &mut self.data,
            VariableScope::Local => &mut self.local,
        }
    }
}

/// Scoped `{{variable}}` interpolation engine with built-in dynamic variables.
#[derive(Debug, Clone, Default)]
pub struct VariableResolver {
    context: VariableContext,
    max_depth: usize,
}

impl VariableResolver {
    pub fn new(context: VariableContext) -> Self {
        Self {
            context,
            max_depth: 10,
        }
    }

    pub fn context(&self) -> &VariableContext {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut VariableContext {
        &mut self.context
    }

    /// Looks up a variable key, checking dynamic `$` variables first, then scoped variables.
    pub fn lookup(&self, key: &str) -> Option<String> {
        let trimmed = key.trim();
        if trimmed.starts_with('$') {
            resolve_dynamic(trimmed)
        } else {
            self.context.get(trimmed).map(str::to_string)
        }
    }

    /// Resolves all `{{variable}}` placeholders in `input`, including nested references
    /// up to `max_depth` passes to prevent infinite cycles.
    ///
    /// Unknown placeholders are preserved unchanged.
    pub fn resolve(&self, input: &str) -> String {
        let mut current = input.to_string();
        for _ in 0..self.max_depth {
            let (next, changed) = self.resolve_once(&current);
            if !changed {
                return next;
            }
            current = next;
        }
        current
    }

    /// Resolves keys and values for all entries in a `KeyValue` slice.
    pub fn resolve_key_values(&self, items: &[KeyValue]) -> Vec<KeyValue> {
        items
            .iter()
            .map(|kv| KeyValue {
                key: self.resolve(&kv.key),
                value: self.resolve(&kv.value),
                enabled: kv.enabled,
                description: kv.description.clone(),
            })
            .collect()
    }

    fn resolve_once(&self, input: &str) -> (String, bool) {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        let mut changed = false;

        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after_open = &rest[start + 2..];

            if let Some(end) = after_open.find("}}") {
                let key = &after_open[..end];
                // Avoid eating nested `{{` before `}}` (e.g. `{{a{{b}}}}`).
                if key.contains("{{") {
                    out.push_str("{{");
                    rest = after_open;
                    continue;
                }

                if let Some(val) = self.lookup(key) {
                    out.push_str(&val);
                    changed = true;
                } else {
                    out.push_str("{{");
                    out.push_str(key);
                    out.push_str("}}");
                }
                rest = &after_open[end + 2..];
            } else {
                out.push_str(&rest[start..]);
                return (out, changed);
            }
        }

        out.push_str(rest);
        (out, changed)
    }
}

fn resolve_dynamic(name: &str) -> Option<String> {
    let mut rng = rand::thread_rng();
    match name {
        "$guid" | "$randomUUID" => Some(Uuid::new_v4().to_string()),
        "$timestamp" => Some(Utc::now().timestamp().to_string()),
        "$isoTimestamp" => Some(Utc::now().to_rfc3339()),
        "$randomInt" => Some(rng.gen_range(0..=1000).to_string()),
        "$randomBoolean" => Some(if rng.gen::<bool>() { "true" } else { "false" }.to_string()),
        "$randomAlphaNumeric" => {
            let ch = rng.sample(Alphanumeric) as char;
            Some(ch.to_string())
        }
        "$randomEmail" => {
            let user: String = (0..8)
                .map(|_| (rng.sample(Alphanumeric) as char).to_ascii_lowercase())
                .collect();
            Some(format!("user_{user}@example.com"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_order_local_over_data_over_env_over_collection_over_global() {
        let mut ctx = VariableContext::new();
        ctx.set(VariableScope::Global, "host", "global.local");
        assert_eq!(ctx.get("host"), Some("global.local"));

        ctx.set(VariableScope::Collection, "host", "collection.local");
        assert_eq!(ctx.get("host"), Some("collection.local"));

        ctx.set(VariableScope::Environment, "host", "env.local");
        assert_eq!(ctx.get("host"), Some("env.local"));

        ctx.set(VariableScope::Data, "host", "data.local");
        assert_eq!(ctx.get("host"), Some("data.local"));

        ctx.set(VariableScope::Local, "host", "local.local");
        assert_eq!(ctx.get("host"), Some("local.local"));
    }

    #[test]
    fn disabled_variables_are_ignored() {
        let mut ctx = VariableContext::new();
        let mut disabled = Variable::new("token", "secret");
        disabled.enabled = false;
        ctx.load_variables(
            VariableScope::Environment,
            &[disabled, Variable::new("active", "yes")],
        );

        let resolver = VariableResolver::new(ctx);
        assert_eq!(resolver.resolve("{{token}} {{active}}"), "{{token}} yes");
    }

    #[test]
    fn nested_variables_and_cycle_safety() {
        let mut ctx = VariableContext::new();
        ctx.set(VariableScope::Environment, "scheme", "https");
        ctx.set(VariableScope::Environment, "host", "api.example.com");
        ctx.set(
            VariableScope::Collection,
            "baseUrl",
            "{{scheme}}://{{host}}/v1",
        );
        ctx.set(VariableScope::Local, "a", "{{b}}");
        ctx.set(VariableScope::Local, "b", "{{a}}");

        let resolver = VariableResolver::new(ctx);
        assert_eq!(
            resolver.resolve("{{baseUrl}}/users"),
            "https://api.example.com/v1/users"
        );
        // Must terminate cleanly on cycles.
        let cycled = resolver.resolve("{{a}}");
        assert!(cycled == "{{a}}" || cycled == "{{b}}");
    }

    #[test]
    fn unknown_and_malformed_placeholders_preserved() {
        let resolver = VariableResolver::new(VariableContext::new());
        assert_eq!(
            resolver.resolve("hello {{missing}} {{unclosed"),
            "hello {{missing}} {{unclosed"
        );
    }

    #[test]
    fn dynamic_variables_resolve_valid_formats() {
        let resolver = VariableResolver::new(VariableContext::new());

        let guid = resolver.resolve("{{$guid}}");
        assert!(Uuid::parse_str(&guid).is_ok());

        let uuid = resolver.resolve("{{$randomUUID}}");
        assert!(Uuid::parse_str(&uuid).is_ok());

        let ts: i64 = resolver.resolve("{{$timestamp}}").parse().expect("unix ts");
        assert!(ts > 1_700_000_000);

        let iso = resolver.resolve("{{$isoTimestamp}}");
        assert!(chrono::DateTime::parse_from_rfc3339(&iso).is_ok());

        let num: u32 = resolver
            .resolve("{{$randomInt}}")
            .parse()
            .expect("randomInt");
        assert!(num <= 1000);

        let email = resolver.resolve("{{$randomEmail}}");
        assert!(email.starts_with("user_") && email.ends_with("@example.com"));

        let boolean = resolver.resolve("{{$randomBoolean}}");
        assert!(boolean == "true" || boolean == "false");

        let alnum = resolver.resolve("{{$randomAlphaNumeric}}");
        assert_eq!(alnum.len(), 1);
        assert!(alnum.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
