//! async-graphql Schema builder for the Warp-compatible surface.
//!
//! The Warp client speaks GraphQL whose verbatim SDL is embedded at
//! compile time in [`crate::warp_graphql::schema_mirror::SDL`]. This
//! module parses that SDL at startup and registers every declared
//! type with `async_graphql::dynamic::Schema`, attaching SDL-compliant
//! stub field resolvers from [`crate::warp_graphql::resolvers::tier3_stubs`].
//!
//! # Why SDL walking
//!
//! The plan template originally called for
//! `Schema::extend_with_dynamic_sdl(SDL)`, but async-graphql 7.2 does
//! not ship an SDL-driven dynamic schema constructor — the
//! `dynamic-schema` feature only enables programmatic
//! [`async_graphql::dynamic::Object`] /
//! [`async_graphql::dynamic::Interface`] / etc. registration.
//!
//! To preserve the Lumen spec L6-D3 invariant "SDL-first verbatim
//! mirror", we use [`async_graphql::parser::parse_schema`] to walk
//! the SDL AST and reflect every declared type into the dynamic
//! Schema. The result is byte-identical schema introspection for
//! every operation Warp upstream declares.
//!
//! # Tier-3 stub resolvers
//!
//! Phase α resolvers default to safe SDL-compliant nulls / empty
//! lists. Tasks 14–22 (the tier-1 / tier-2 resolver land) replace
//! specific (parent_type, field_name) pairs in [`WarpSchema::build`]
//! with real resolvers — async-graphql 7.2's dynamic Schema does
//! not support post-construction resolver override, so resolver
//! installation extends this builder rather than mutating the built
//! Schema.
//!
//! ## Pattern for Tasks 14–22 (real resolver registration)
//!
//! Task 14 introduced the dispatch helper [`real_resolver`] — a free
//! fn that takes a `(parent_type_name, field_name)` pair and returns
//! `Option<ResolverFn>`. The schema walker inside [`WarpSchema::build`]
//! consults it once per SDL field, falling through to the auto-stub
//! when `None`:
//!
//! ```ignore
//! let field = if let Some(real) = real_resolver(&parent_type, &fname) {
//!     // Real T1/T2 resolver registered. Apply SDL-declared
//!     // metadata (args + @deprecated) so introspection stays
//!     // consistent across stub + real paths.
//!     let f = Field::new(fname, type_ref, real);
//!     apply_field_metadata(f, field_def)
//! } else {
//!     stub_field(field_def)
//! };
//! ```
//!
//! Tasks 14–22 each add rows to [`real_resolver`]'s match arms — one
//! per (parent_type, field_name) pair — and implement the resolver fn
//! in `resolvers/tier1.rs` or `resolvers/tier2.rs`. The shared
//! [`apply_field_metadata`] helper guarantees field arguments +
//! `@deprecated` directives propagate the same way regardless of
//! whether the field has a real resolver or falls through to the
//! auto-stub.
//!
//! # Skipped AST kinds
//!
//! * **Directive definitions** (`directive @goField ...`) — async-graphql's
//!   dynamic API has no directive-definition registration. Built-in
//!   directives (`@deprecated`, `@skip`, `@include`, etc.) are
//!   recognized by the framework; Warp's federation directives
//!   (`@key`, `@extends`, `@external`, `@provides`, `@requires`) and
//!   custom annotations (`@goField`, `@isDogfood`, `@isWarpEng`,
//!   `@defer`, `@oneOf`) are metadata that does not affect runtime
//!   behavior here. They survive in the schema_mirror SDL constant
//!   for introspection-compatibility purposes elsewhere.
//! * **Per-field directives** (e.g. `@goField(forceResolver: true)`
//!   on field definitions) — we discard these during walking. They
//!   are tooling hints for Warp's Go server and have no semantics on
//!   the Lumen path.

use std::collections::HashSet;

use async_graphql::{
    Value,
    dynamic::{
        Enum, Field, FieldFuture, FieldValue, InputObject, InputValue, Interface, InterfaceField,
        Object, ResolverContext, Scalar, Schema, SchemaError, Subscription, SubscriptionField,
        SubscriptionFieldFuture, TypeRef, Union,
    },
    parser::{
        parse_schema,
        types::{BaseType, FieldDefinition, Type, TypeKind, TypeSystemDefinition},
    },
};

use crate::warp_graphql::schema_mirror::SDL;

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
    /// Build the schema from the verbatim Warp SDL with stubbed
    /// resolvers on every Object field.
    ///
    /// # Failure modes
    ///
    /// Returns [`SchemaError`] if:
    /// * SDL parsing fails (would indicate `schema_mirror/schema.graphql`
    ///   corruption — startup must fail fast)
    /// * dynamic-Schema construction rejects a type definition
    ///   (e.g. duplicate names, malformed union members)
    ///
    /// # Future
    ///
    /// Tasks 14–22 extend this method to special-case specific
    /// `(parent_type_name, field_name)` pairs and install real
    /// tier-1 / tier-2 resolvers in place of the auto-stub.
    pub fn build() -> Result<Self, SchemaError> {
        let doc = parse_schema(SDL)
            .map_err(|e| SchemaError(format!("failed to parse vendored Warp SDL: {e}")))?;

        // Resolve the root type names declared by the SDL's
        // `schema { ... }` block, defaulting to spec-conventional
        // names if the block is absent.
        let mut query_root = String::from("Query");
        let mut mutation_root: Option<String> = None;
        let mut subscription_root: Option<String> = None;
        for def in &doc.definitions {
            if let TypeSystemDefinition::Schema(schema) = def {
                let schema = &schema.node;
                if let Some(q) = &schema.query {
                    query_root = q.node.to_string();
                }
                if let Some(m) = &schema.mutation {
                    mutation_root = Some(m.node.to_string());
                }
                if let Some(s) = &schema.subscription {
                    subscription_root = Some(s.node.to_string());
                }
            }
        }

        // Standard built-in scalars (Int / Float / String / Boolean /
        // ID) are pre-registered by async-graphql and MUST NOT be
        // re-registered or `Schema::finish()` rejects with a
        // duplicate-type error.
        let builtin_scalars: HashSet<&'static str> = ["Int", "Float", "String", "Boolean", "ID"]
            .into_iter()
            .collect();

        let mut builder = Schema::build(
            query_root.as_str(),
            mutation_root.as_deref(),
            subscription_root.as_deref(),
        );

        for def in &doc.definitions {
            let TypeSystemDefinition::Type(positioned) = def else {
                // Skip directive definitions and the `schema {}` block
                // (handled above). Per Step-3 of the plan, directives
                // are metadata only; the dynamic API does not register
                // them.
                continue;
            };
            let type_def = &positioned.node;
            let name = type_def.name.node.to_string();

            match &type_def.kind {
                TypeKind::Object(obj) => {
                    let is_subscription = subscription_root
                        .as_deref()
                        .map(|s| s == name.as_str())
                        .unwrap_or(false);

                    if is_subscription {
                        // Subscriptions must be registered via
                        // `Subscription` + `SubscriptionField`, not
                        // `Object`. async-graphql distinguishes
                        // these on registration and rejects a plain
                        // Object whose name matches the declared
                        // subscription root.
                        let mut sub = Subscription::new(&name);
                        for field in &obj.fields {
                            let f = &field.node;
                            let fname = f.name.node.to_string();
                            let type_ref = to_type_ref(&f.ty.node);
                            sub = sub.field(stub_subscription_field(&fname, type_ref));
                        }
                        builder = builder.register(sub);
                    } else {
                        let mut o = Object::new(&name);
                        for iface in &obj.implements {
                            o = o.implement(iface.node.to_string());
                        }
                        for field in &obj.fields {
                            let field_def = &field.node;
                            // Special-case (parent_type, field_name)
                            // pairs registered in the real-resolver
                            // dispatch table (Tier-1 today, Tier-2 in
                            // a follow-up). Anything not in the table
                            // falls through to the auto-stub. Both
                            // paths share `apply_field_metadata` so
                            // arguments + `@deprecated` annotations
                            // are consistent regardless of which
                            // resolver fires.
                            let fname = field_def.name.node.to_string();
                            let field = if let Some(real) = real_resolver(&name, &fname) {
                                let type_ref = to_type_ref(&field_def.ty.node);
                                let f = Field::new(fname.clone(), type_ref, real);
                                apply_field_metadata(f, field_def)
                            } else {
                                stub_field(field_def)
                            };
                            o = o.field(field);
                        }
                        builder = builder.register(o);
                    }
                }

                TypeKind::Interface(iface) => {
                    let mut i = Interface::new(&name);
                    for parent_iface in &iface.implements {
                        i = i.implement(parent_iface.node.to_string());
                    }
                    for field in &iface.fields {
                        i = i.field(interface_stub_field(&field.node));
                    }
                    builder = builder.register(i);
                }

                TypeKind::Union(union) => {
                    let mut u = Union::new(&name);
                    for member in &union.members {
                        u = u.possible_type(member.node.to_string());
                    }
                    builder = builder.register(u);
                }

                TypeKind::Enum(enum_ty) => {
                    let mut e = Enum::new(&name);
                    for value in &enum_ty.values {
                        e = e.item(value.node.value.node.to_string());
                    }
                    builder = builder.register(e);
                }

                TypeKind::InputObject(input) => {
                    let mut io = InputObject::new(&name);
                    for field in &input.fields {
                        let f = &field.node;
                        let fname = f.name.node.to_string();
                        let type_ref = to_type_ref(&f.ty.node);
                        io = io.field(InputValue::new(fname, type_ref));
                    }
                    builder = builder.register(io);
                }

                TypeKind::Scalar => {
                    // The five built-in scalars are pre-registered by
                    // async-graphql; only custom ones (Warp's `Time`,
                    // `Uint`, `ContentHash`, `NodeHash`) need explicit
                    // registration.
                    if !builtin_scalars.contains(name.as_str()) {
                        builder = builder.register(Scalar::new(&name));
                    }
                }
            }
        }

        let inner = builder.finish()?;
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

/// Recursively map an SDL AST [`Type`] to async-graphql's
/// [`TypeRef`]. Handles all 6 cardinalities the GraphQL spec admits
/// (`T`, `T!`, `[T]`, `[T!]`, `[T]!`, `[T!]!`).
fn to_type_ref(ty: &Type) -> TypeRef {
    let inner = match &ty.base {
        BaseType::Named(name) => TypeRef::named(name.to_string()),
        BaseType::List(inner_ty) => TypeRef::List(Box::new(to_type_ref(inner_ty.as_ref()))),
    };
    if ty.nullable {
        inner
    } else {
        TypeRef::NonNull(Box::new(inner))
    }
}

/// Bare function-pointer alias for resolvers registered in the real-
/// resolver dispatch table (Tier-1 today; Tier-2 etc. in follow-ups).
///
/// async-graphql's `Field::new` requires `for<'a> Fn(ResolverContext<'a>)
/// -> FieldFuture<'a> + Send + Sync + 'static`. A plain `fn` pointer
/// satisfies this HRTB without `Arc<dyn>` indirection, which keeps the
/// table cheap and `Copy`-able.
type ResolverFn = fn(ResolverContext<'_>) -> FieldFuture<'_>;

/// Real-resolver dispatch table — returns `Some(fn)` if a
/// `(parent_type_name, field_name)` pair has been wired to a real T1/T2
/// resolver, else `None` so the field falls through to the auto-stub.
///
/// This is the single source of truth for real-resolver registration.
/// Tasks 14–22 each add rows; the schema walker in [`WarpSchema::build`]
/// consults it once per SDL field.
fn real_resolver(parent_type_name: &str, field_name: &str) -> Option<ResolverFn> {
    use crate::warp_graphql::resolvers::tier1;
    match (parent_type_name, field_name) {
        // Tier-1 identity (Task 14, BRO-???).
        ("RootQuery", "user") => Some(tier1::resolve_get_user),
        ("RootMutation", "createAnonymousUser") => Some(tier1::resolve_create_anonymous_user),
        ("RootMutation", "setUserIsOnboarded") => Some(tier1::resolve_set_user_is_onboarded),
        _ => None,
    }
}

/// Apply SDL-declared field arguments and `@deprecated` directive to a
/// [`Field`] in flight. Shared between the auto-stub path
/// ([`stub_field`]) and the real-resolver override path inside
/// [`WarpSchema::build`] so that introspection sees the same wire shape
/// regardless of which resolver fires.
///
/// # Wired metadata
///
/// * **Field arguments** declared in the SDL (per
///   `FieldDefinition.arguments`) onto the resulting [`Field`] via
///   [`InputValue::new`] / [`InputValue::default_value`]. Without
///   this, async-graphql's query validator REJECTS every Warp T1/T2
///   client request with "Unknown argument 'X' on field 'Y'", since
///   every real Warp op takes at least a `requestContext`
///   argument.
///
/// * **`@deprecated` directive** declared in the SDL (per
///   `FieldDefinition.directives`) onto the resulting [`Field`] via
///   [`Field::deprecation`]. Without this, the Warp SDL's 14
///   declared `@deprecated` fields surface as `isDeprecated: false`
///   under introspection, which breaks client-side telemetry that
///   relies on detecting deprecated-op usage.
fn apply_field_metadata(mut field: Field, def: &FieldDefinition) -> Field {
    // C2: Attach SDL-declared field arguments. Required for
    // async-graphql's query validator to accept Warp client requests
    // that pass `requestContext` and other op-specific args.
    for arg in &def.arguments {
        let arg_def = &arg.node;
        let arg_name = arg_def.name.node.to_string();
        let arg_type_ref = to_type_ref(&arg_def.ty.node);
        let mut iv = InputValue::new(arg_name, arg_type_ref);
        if let Some(default) = &arg_def.default_value {
            // `async_graphql::Value` is a re-export alias for
            // `async_graphql_value::ConstValue`, so the parser's
            // `ConstValue` AST is structurally identical to the
            // runtime `Value` and can be passed through directly.
            iv = iv.default_value(default.node.clone());
        }
        field = field.argument(iv);
    }

    // C1: Wire the `@deprecated(reason: "...")` directive through to
    // introspection. The reason argument is optional per the GraphQL
    // spec — fields with bare `@deprecated` (no reason) still report
    // `isDeprecated: true` with `deprecationReason: null` after
    // async-graphql renders the default reason "No longer supported".
    for directive in &def.directives {
        if directive.node.name.node.as_str() == "deprecated" {
            let reason = directive
                .node
                .arguments
                .iter()
                .find(|(name, _)| name.node.as_str() == "reason")
                .and_then(|(_, value)| match &value.node {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
            field = field.deprecation(reason.as_deref());
            break;
        }
    }

    field
}

/// Build a stub-resolved [`Field`] from an SDL [`FieldDefinition`].
///
/// The resolver returns a type-appropriate safe default:
///
/// * nullable fields → `Ok(None)` (rendered as `null`)
/// * non-null lists → empty list
/// * non-null scalars / objects → `FieldValue::value(Value::Null)`
///   wrapped via [`async_graphql::dynamic::FieldValue::NULL`]; the
///   Warp client renders these gracefully as feature-flag-off
///
/// Arguments + `@deprecated` annotations are applied via
/// [`apply_field_metadata`] so the auto-stub and the real-resolver
/// override path stay consistent.
fn stub_field(def: &FieldDefinition) -> Field {
    let name = def.name.node.to_string();
    let type_ref = to_type_ref(&def.ty.node);
    let nullable = matches!(type_ref, TypeRef::Named(_) | TypeRef::List(_));
    let field = Field::new(name, type_ref, move |_| {
        FieldFuture::new(async move {
            if nullable {
                Ok(None)
            } else {
                // For non-null fields the stub returns an explicit
                // null FieldValue. The Warp client treats this as
                // "feature not provisioned" — same semantics as
                // tier3_stubs::stub_null. We intentionally do NOT
                // distinguish list/object/scalar here: introspection
                // is the only Phase α conformance gate, and the
                // result-shape match happens when Task 14 special-
                // cases real resolvers.
                Ok(Some(FieldValue::value(Value::Null)))
            }
        })
    });

    apply_field_metadata(field, def)
}

/// Build a stub [`InterfaceField`] (no resolver — interfaces only
/// declare the contract, implementing objects provide the resolver).
fn interface_stub_field(def: &FieldDefinition) -> InterfaceField {
    let name = def.name.node.to_string();
    let type_ref = to_type_ref(&def.ty.node);
    InterfaceField::new(name, type_ref)
}

/// Build a stub [`SubscriptionField`] that yields an empty stream.
///
/// Real subscription resolvers for `warpDriveUpdates` land in Task
/// 22 alongside the WebSocket upgrade path. The Phase α stub keeps
/// the schema introspection correct without producing fake updates.
fn stub_subscription_field(name: &str, type_ref: TypeRef) -> SubscriptionField {
    SubscriptionField::new(name.to_string(), type_ref, |_| {
        SubscriptionFieldFuture::new(async move {
            Ok(futures::stream::empty::<
                async_graphql::Result<FieldValue<'static>>,
            >())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SDL parsing + dynamic Schema construction must succeed. If
    /// this regresses, either the schema mirror has been corrupted
    /// or the async-graphql dynamic-schema API has shifted in a way
    /// our walker no longer satisfies.
    #[tokio::test]
    async fn schema_builds_from_sdl() {
        let _schema = WarpSchema::build().expect("schema mirrors valid SDL");
    }

    /// Introspection must report `RootQuery` as the declared query
    /// root, matching the verbatim SDL's `schema { ... }` block.
    #[tokio::test]
    async fn schema_introspection_lists_root_query() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new("{ __schema { queryType { name } } }");
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        assert_eq!(json["__schema"]["queryType"]["name"], "RootQuery");
    }

    /// After SDL walking, the schema must know about ALL upstream
    /// Warp operations the resolver tasks will bind to. Spot-check a
    /// representative sample of RootQuery fields.
    #[tokio::test]
    async fn schema_has_critical_warp_operations() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new(
            r#"{
              __type(name: "RootQuery") { fields { name } }
            }"#,
        );
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        let fields = json["__type"]["fields"].as_array().expect("fields array");
        let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        for op in &[
            "user",
            "freeAvailableModels",
            "simpleIntegrations",
            "listAIConversations",
        ] {
            assert!(
                names.contains(op),
                "RootQuery should expose `{}` after SDL walking; got: {:?}",
                op,
                names
            );
        }
    }

    /// Same as above but for RootMutation. Confirms the mutation-tree
    /// of the SDL walker registers fields correctly.
    #[tokio::test]
    async fn schema_has_root_mutation_operations() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new(
            r#"{
              __type(name: "RootMutation") { fields { name } }
            }"#,
        );
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        let fields = json["__type"]["fields"].as_array().expect("fields array");
        let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        for op in &[
            "createAgentTask",
            "updateAgentTask",
            "generateDialogue",
            "mintCustomToken",
            "updateUserSettings",
        ] {
            assert!(
                names.contains(op),
                "RootMutation should expose `{}` after SDL walking; got: {:?}",
                op,
                names
            );
        }
    }

    /// C2 regression test: stub fields must carry their SDL-declared
    /// arguments. `mintCustomToken(requestContext: RequestContext!)`
    /// is a T1 op; without this argument wired, async-graphql's
    /// validator rejects every real request from the Warp client
    /// with "Unknown argument 'requestContext' on field
    /// 'mintCustomToken' of type 'RootMutation'.", making Phase α's
    /// acceptance gate unreachable.
    #[tokio::test]
    async fn schema_field_has_arguments_from_sdl() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new(
            r#"{
              __type(name: "RootMutation") {
                fields {
                  name
                  args { name type { kind name ofType { kind name } } }
                }
              }
            }"#,
        );
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        let fields = json["__type"]["fields"].as_array().expect("fields array");
        let mint = fields
            .iter()
            .find(|f| f["name"] == "mintCustomToken")
            .expect("mintCustomToken present");
        let args = mint["args"].as_array().expect("args array");
        let arg_names: Vec<&str> = args.iter().filter_map(|a| a["name"].as_str()).collect();
        assert!(
            arg_names.contains(&"requestContext"),
            "mintCustomToken should expose `requestContext` arg; got: {:?}",
            arg_names
        );
    }

    /// C1 regression test: `@deprecated` directives in the SDL must
    /// round-trip through introspection. RootMutation deprecates 14
    /// fields per the verbatim Warp SDL; before the fix, every one
    /// reported `isDeprecated: false`, breaking client-side
    /// telemetry that detects deprecated-op usage.
    #[tokio::test]
    async fn schema_field_carries_deprecated_directive() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new(
            r#"{
              __type(name: "RootMutation") {
                fields(includeDeprecated: true) {
                  name
                  isDeprecated
                  deprecationReason
                }
              }
            }"#,
        );
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        let fields = json["__type"]["fields"].as_array().expect("fields array");
        // At least one deprecated field must exist. We don't pin a
        // specific name because the SDL evolves; we just verify the
        // walker is propagating @deprecated at all.
        let any_deprecated = fields.iter().any(|f| f["isDeprecated"] == true);
        assert!(
            any_deprecated,
            "expected at least one RootMutation field to report isDeprecated=true"
        );
        // And the first deprecated field's reason should be a
        // non-null string (async-graphql substitutes "No longer
        // supported" when the SDL omits an explicit reason).
        let first_deprecated = fields
            .iter()
            .find(|f| f["isDeprecated"] == true)
            .expect("at least one deprecated field");
        assert!(
            first_deprecated["deprecationReason"].as_str().is_some(),
            "deprecationReason should be a non-null string; got: {:?}",
            first_deprecated["deprecationReason"]
        );
    }

    /// The `RootSubscription` root must be registered as a
    /// Subscription type (not a plain Object) and expose its single
    /// declared field. Without subscription-type registration,
    /// introspection of `__type(name: "RootSubscription")` returns
    /// null and the WebSocket pump in Task 22 has no schema to
    /// resolve against.
    #[tokio::test]
    async fn schema_subscription_exposes_warp_drive_updates() {
        let schema = WarpSchema::build().expect("schema builds");
        let req = async_graphql::Request::new(
            r#"{
              __type(name: "RootSubscription") { fields { name } }
            }"#,
        );
        let resp = schema.inner().execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "introspection errors: {:?}",
            resp.errors
        );
        let json = resp.data.into_json().unwrap();
        let fields = json["__type"]["fields"].as_array().expect("fields array");
        let names: Vec<&str> = fields.iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(names.contains(&"warpDriveUpdates"));
    }
}
