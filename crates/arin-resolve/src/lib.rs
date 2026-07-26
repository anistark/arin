//! Grounding resolvers.
//!
//! A resolver turns `"the Submit button"` into coordinates. The `Resolver` trait is
//! defined in `arin-core` alongside the other platform seams. This crate holds the
//! registry that maps configuration names to implementations, plus the adapters
//! themselves.
//!
//! # Status
//!
//! The registry exists, but there are no adapters yet. They land in 0.3, when the query
//! form of `point` and `highlight` goes live. Two are planned: a hosted computer-use
//! model with a bring-your-own key, and a local grounding model in 0.5 that removes the
//! API key and the screenshot egress along with it.
//!
//! # Egress
//!
//! A resolver is the only part of Arin that can send anything off the machine. Every
//! implementation must report [`arin_core::Resolver::is_remote`] honestly, because that
//! is what the consent prompt is built on. A resolver that quietly returns `false` while
//! making a network call defeats the entire privacy story.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use arin_core::Resolver;
use std::collections::HashMap;
use std::sync::Arc;

/// Maps configured names to resolver implementations.
///
/// The daemon holds at most one active resolver. The registry exists so adding a
/// community adapter is a registration call rather than a patch to core.
#[derive(Default)]
pub struct Registry {
    resolvers: HashMap<String, Arc<dyn Resolver>>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a resolver under its own [`arin_core::Resolver::name`].
    ///
    /// Returns the previous registration under that name, if any.
    pub fn register(&mut self, resolver: Arc<dyn Resolver>) -> Option<Arc<dyn Resolver>> {
        let name = resolver.name().to_owned();
        tracing::debug!(%name, remote = resolver.is_remote(), "registered resolver");
        self.resolvers.insert(name, resolver)
    }

    /// Look up a resolver by configured name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Resolver>> {
        self.resolvers.get(name)
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.resolvers.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Whether any resolver is registered.
    pub fn is_empty(&self) -> bool {
        self.resolvers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arin_core::{Frame, Resolution, Result};
    use futures::future::BoxFuture;

    struct Stub(&'static str, bool);

    impl Resolver for Stub {
        fn name(&self) -> &str {
            self.0
        }

        fn is_remote(&self) -> bool {
            self.1
        }

        fn resolve<'a>(
            &'a self,
            _query: &'a str,
            _frame: &'a Frame,
        ) -> BoxFuture<'a, Result<Resolution>> {
            Box::pin(async { unreachable!("stub resolver is never invoked") })
        }
    }

    #[test]
    fn registers_under_its_own_name() {
        let mut registry = Registry::new();
        assert!(registry.is_empty());

        registry.register(Arc::new(Stub("local-cua", false)));
        registry.register(Arc::new(Stub("hosted-cua", true)));

        assert_eq!(registry.names(), vec!["hosted-cua", "local-cua"]);
        assert!(registry.get("local-cua").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn re_registering_replaces() {
        let mut registry = Registry::new();
        registry.register(Arc::new(Stub("cua", false)));
        let previous = registry.register(Arc::new(Stub("cua", true)));

        assert!(previous.is_some());
        assert_eq!(registry.names().len(), 1);
        assert!(registry.get("cua").unwrap().is_remote());
    }
}
