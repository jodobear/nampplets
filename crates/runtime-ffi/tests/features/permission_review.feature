Feature: Runtime FFI permission admission is exact-build bounded

  The native boundary must project Rust's review and refusal for one verified
  build without reconstructing its authority or admitting untrusted execution.

  Background:
    Given a verified published manifest with no signed requires tags
    And its hash-matching entry document declares bounded napplet requirements

  Scenario: Embedded requirements reach exact-build review
    When the exact build is requested through the runtime FFI permission facade
    Then the review contains exactly the authenticated normalized domains
    And the review principal is bound to manifest author, dTag, and aggregateHash

  Scenario: Missing grants refuse before a session crosses the boundary
    When launch is attempted without granting the required domains
    Then no session crosses the runtime FFI boundary
    And the exact build receives typed bridge refusal evidence
