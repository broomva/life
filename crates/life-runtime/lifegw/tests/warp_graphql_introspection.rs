//! Conformance test: introspection result matches the verbatim SDL.
//!
//! If this test fails, our SDL mirror has drifted from upstream Warp's
//! schema, OR our resolver bindings violated SDL types. Either is a
//! M7-W contract violation per Lumen spec L6-D3.

use async_graphql::parser::types::ServiceDocument;
use lifegw::warp_graphql::adapter::WarpSchema;
use lifegw::warp_graphql::schema_mirror::SDL;

#[tokio::test]
async fn introspection_returns_full_schema() {
    let schema = WarpSchema::build().expect("schema builds");

    let req = async_graphql::Request::new(INTROSPECTION_QUERY);
    let resp = schema.inner().execute(req).await;
    assert!(
        resp.errors.is_empty(),
        "introspection errors: {:?}",
        resp.errors
    );
}

#[test]
fn sdl_mirror_parses_as_valid_graphql_document() {
    let parsed: ServiceDocument = async_graphql::parser::parse_schema(SDL).expect("SDL parses");

    // Sanity: schema declares query/mutation/subscription roots and the
    // RootQuery/RootMutation/RootSubscription types exist.
    let sdl_text = SDL;
    assert!(sdl_text.contains("type RootQuery"), "SDL missing RootQuery");
    assert!(
        sdl_text.contains("type RootMutation"),
        "SDL missing RootMutation"
    );
    assert!(
        sdl_text.contains("type RootSubscription"),
        "SDL missing RootSubscription"
    );

    // The parsed document should have at least 100 definitions
    // (rough sanity — the actual schema has hundreds).
    assert!(
        parsed.definitions.len() >= 100,
        "SDL parsed to only {} definitions; mirror may be truncated",
        parsed.definitions.len()
    );
}

#[test]
fn sdl_mirror_size_within_expected_range() {
    // Upstream Warp schema is ~4,133 lines. We accept ±5% drift between
    // rebases as normal upstream evolution; anything more triggers a
    // manual rebase ticket.
    let line_count = SDL.lines().count();
    assert!(
        (3920..=4340).contains(&line_count),
        "SDL line count {} outside expected range; check upstream pin",
        line_count
    );
}

#[test]
fn sdl_mirror_includes_critical_operations() {
    // Tier 1 + Tier 2 operations the resolvers will bind to.
    // Names match upstream Warp's mixed convention (bare-noun for
    // user/freeAvailableModels/simpleIntegrations; get-prefixed for
    // getRelevantFragments/getCloudEnvironments). See 2026-05-11
    // Pre-flight Update Round 3 in the plan.
    const REQUIRED_OPS: &[&str] = &[
        // Mutations
        "createAgentTask",
        "updateAgentTask",
        "generateDialogue",
        "generateCommands",
        "generateCodeEmbeddings",
        "createAnonymousUser",
        "setUserIsOnboarded",
        "mintCustomToken",
        "issueTaskIdentityToken",
        "generateApiKey",
        "expireApiKey",
        "updateUserSettings",
        // Queries (mixed convention)
        "user",
        "freeAvailableModels",
        "simpleIntegrations",
        "codebaseContextConfig",
        "listAIConversations",
        "getRelevantFragments",
        "rerankFragments",
        // Subscription
        "warpDriveUpdates",
    ];

    for op in REQUIRED_OPS {
        assert!(
            SDL.contains(op),
            "SDL missing required operation '{}'; upstream may have renamed it",
            op
        );
    }
}

#[tokio::test]
async fn introspection_lists_root_query_fields() {
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

    // Spot-check the T1 + T2 op set
    for op in &[
        "user",
        "freeAvailableModels",
        "simpleIntegrations",
        "listAIConversations",
    ] {
        assert!(
            names.contains(op),
            "RootQuery should expose `{}`; got: {:?}",
            op,
            names
        );
    }
}

#[tokio::test]
async fn introspection_lists_root_mutation_fields() {
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
    ] {
        assert!(
            names.contains(op),
            "RootMutation should expose `{}`; got: {:?}",
            op,
            names
        );
    }
}

#[tokio::test]
async fn introspection_arguments_are_present() {
    // Task 10.5 fix-up wired SDL field arguments through the auto-stub.
    // Verify that introspection actually returns them — without args,
    // async-graphql's validator would reject any T1 op call from a
    // Warp client.
    let schema = WarpSchema::build().expect("schema builds");
    let req = async_graphql::Request::new(
        r#"{
          __type(name: "RootMutation") {
            fields {
              name
              args { name }
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
        "mintCustomToken must expose `requestContext` arg; got: {:?}",
        arg_names
    );
}

const INTROSPECTION_QUERY: &str = r#"
{
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      name
      kind
    }
  }
}
"#;
