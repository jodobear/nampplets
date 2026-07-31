use super::*;

#[test]
fn oversized_system_refusal_keeps_a_typed_bounded_result() {
    let fallback = bounded_refusal_message(
        NapDomain::Outbox,
        "request-1",
        OVERSIZED_REFUSAL_ERROR,
        usize::MAX,
    )
    .expect("fallback is serializable");

    let response = bounded_refusal_message(
        NapDomain::Outbox,
        "request-1",
        &"upstream failure ".repeat(128),
        fallback.byte_len(),
    )
    .expect("validated response bound retains the typed fallback");

    let value = response.decode().expect("fallback is valid JSON");
    assert_eq!(value["type"], "outbox.publish.result");
    assert_eq!(value["id"], "request-1");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"], OVERSIZED_REFUSAL_ERROR);
}
