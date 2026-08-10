use mark_session::{
    METHOD_REVIEW_GET, PROTOCOL_VERSION, ProtocolError, Request, Response, ReviewAnchor,
    ReviewAnchorScope, ReviewParams,
};

#[test]
fn request_and_response_json_are_stable() {
    let request = Request::new(
        "req-17",
        METHOD_REVIEW_GET,
        serde_json::to_value(ReviewParams {
            cursor: None,
            limit: Some(200),
            include_comments: false,
            comments_cursor: None,
            comments_limit: None,
            changed_only: false,
        })
        .unwrap(),
    );
    assert_eq!(
        serde_json::to_string_pretty(&request).unwrap(),
        r#"{
  "protocol": 1,
  "id": "req-17",
  "method": "review.get",
  "params": {
    "changed_only": false,
    "cursor": null,
    "include_comments": false,
    "limit": 200
  }
}"#
    );

    let response = Response::failure(
        "req-17",
        ProtocolError::new("anchor_not_found", "line is not in the changeset")
            .with_details(serde_json::json!({"file": "src/auth.rs", "new_line": 42})),
    );
    let value = serde_json::to_value(response).unwrap();
    assert_eq!(value["protocol"], PROTOCOL_VERSION);
    assert_eq!(value["error"]["code"], "anchor_not_found");
    assert_eq!(value["error"]["details"]["new_line"], 42);
}

#[test]
fn file_anchor_scope_is_explicit_and_backward_compatible() {
    let anchor = ReviewAnchor {
        file: "src/lib.rs".to_owned(),
        scope: Some(ReviewAnchorScope::File),
        hunk: None,
        old_line: None,
        new_line: None,
        range: None,
    };
    let value = serde_json::to_value(anchor).unwrap();
    assert_eq!(value["scope"], "file");

    let legacy: ReviewAnchor = serde_json::from_value(serde_json::json!({
        "file": "src/lib.rs",
        "new_line": 1
    }))
    .unwrap();
    assert_eq!(legacy.scope, None);
}

#[test]
fn unknown_request_fields_are_ignored() {
    let request: Request = serde_json::from_value(serde_json::json!({
        "protocol": 1,
        "id": "req-1",
        "method": "review.get",
        "params": {},
        "future": true
    }))
    .unwrap();
    assert_eq!(request.id, "req-1");
}
