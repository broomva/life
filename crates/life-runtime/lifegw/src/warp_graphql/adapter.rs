//! async-graphql Schema builder for the Warp-compatible surface.
//!
//! The Warp client speaks GraphQL whose verbatim SDL is embedded at
//! compile time in [`crate::warp_graphql::schema_mirror::SDL`]. Tasks
//! 11–23 of M7-W.1 register resolvers against the schema; this module
//! constructs the [`async_graphql::dynamic::Schema`] they hang off.
//!
//! # Phase α scaffold (M7-W.1 Task 10)
//!
//! The plan template originally called for `Schema::extend_with_dynamic_sdl(SDL)`,
//! but **async-graphql 7.2 does not ship an SDL parser into the dynamic
//! schema**. The `dynamic-schema` feature only enables programmatic
//! [`async_graphql::dynamic::Object`] / [`async_graphql::dynamic::Interface`]
//! / [`async_graphql::dynamic::Enum`] registration; there is no
//! `from_sdl` / `parse_sdl` / `extend_with_sdl` constructor.
//!
//! For Phase α we therefore build the minimal scaffold required by:
//!
//! 1. **Schema-builder discipline.** `Schema::build("RootQuery",
//!    Some("RootMutation"), Some("RootSubscription"))` matches the
//!    schema mirror's declared root types (see
//!    `schema_mirror/schema.graphql` lines 1–5).
//! 2. **`check_root_types` validity.** Each declared root must exist
//!    in the type registry. `RootQuery` and `RootMutation` must be
//!    `Type::Object` with at least one field; `RootSubscription` must
//!    be `Type::Subscription` (field-count unconstrained).
//! 3. **Introspection.** `{ __schema { queryType { name } } }`
//!    must resolve to `"RootQuery"`. Schema introspection reports the
//!    registered `query_type` name directly, so this works without
//!    registering the rest of the type universe.
//!
//! The single placeholder field on each root is named with a `_lumen`
//! prefix so it cannot collide with any operation in the verbatim
//! schema mirror (Warp's operations are `lowerCamelCase` without
//! underscores). Subsequent tasks (11–23) re-bind RootQuery /
//! RootMutation / RootSubscription with the real fields, at which
//! point the placeholders go away.
//!
//! # Long-term plan
//!
//! Once the resolver tasks (11–23) land, the schema construction will
//! walk every type declared in the SDL mirror and register matching
//! [`async_graphql::dynamic::Object`] stubs, then bind tier-1 / tier-2
//! resolvers per [`crate::warp_graphql::resolvers`]. Tier-3 operations
//! return the safe defaults from
//! [`crate::warp_graphql::resolvers::tier3_stubs`].

use async_graphql::{
    Value,
    dynamic::{Field, FieldFuture, Object, Schema, SchemaError, Subscription, TypeRef},
};

/// Placeholder field name used on each root type during the Phase α
/// scaffold. Prefixed with `_lumen_` so it cannot collide with any
/// operation in the verbatim schema mirror.
const PLACEHOLDER_FIELD: &str = "_lumenPlaceholder";

/// The compiled Warp-compatible schema.
///
/// Wraps [`async_graphql::dynamic::Schema`], which is cheap to clone
/// (`Arc` internally). Built once at lifegw startup and shared across
/// every Warp GraphQL request.
#[derive(Clone)]
pub struct WarpSchema {
    inner: Schema,
}

impl WarpSchema {
    /// Build the schema with the verbatim SDL's root type names and
    /// Phase α placeholder fields.
    ///
    /// Returns [`SchemaError`] if construction fails (e.g. duplicate
    /// type names, root-type mismatches). A startup-time failure here
    /// would indicate corruption in the resolver-binding logic and
    /// must surface before lifegw serves traffic.
    ///
    /// # Future
    ///
    /// Tasks 14–22 will replace the placeholder fields with the real
    /// Warp operations bound to tier-1 / tier-2 / tier-3 resolvers.
    pub fn build() -> Result<Self, SchemaError> {
        let query = Object::new("RootQuery").field(Field::new(
            PLACEHOLDER_FIELD,
            TypeRef::named(TypeRef::STRING),
            |_| {
                FieldFuture::new(async move { Ok(Some(Value::from("lumen-phase-alpha-scaffold"))) })
            },
        ));

        let mutation = Object::new("RootMutation").field(Field::new(
            PLACEHOLDER_FIELD,
            TypeRef::named(TypeRef::STRING),
            |_| {
                FieldFuture::new(async move { Ok(Some(Value::from("lumen-phase-alpha-scaffold"))) })
            },
        ));

        let subscription = Subscription::new("RootSubscription");

        let inner = Schema::build("RootQuery", Some("RootMutation"), Some("RootSubscription"))
            .register(query)
            .register(mutation)
            .register(subscription)
            .finish()?;

        Ok(Self { inner })
    }

    /// Borrow the inner [`async_graphql::dynamic::Schema`].
    ///
    /// Used by `crate::warp_graphql::route` (Task 11) to invoke
    /// [`async_graphql::dynamic::Schema::execute`] on incoming Warp
    /// requests.
    pub fn inner(&self) -> &Schema {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema construction itself must succeed. If this regresses
    /// it indicates either the schema mirror has been corrupted or the
    /// async-graphql dynamic-schema API has shifted in a way the
    /// scaffold no longer satisfies.
    #[test]
    fn schema_builds_from_sdl() {
        let _schema = WarpSchema::build().expect("schema mirrors valid SDL");
    }

    /// Introspection over the schema must report the Warp upstream's
    /// root type names. The verbatim SDL declares
    /// `schema { query: RootQuery, mutation: RootMutation, subscription: RootSubscription }`
    /// (see `schema_mirror/schema.graphql:1-5`).
    #[tokio::test]
    async fn schema_introspection_lists_query_type() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new("{ __schema { queryType { name } } }");
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().expect("introspection data is JSON");
        assert_eq!(json["__schema"]["queryType"]["name"], "RootQuery");
    }
}
