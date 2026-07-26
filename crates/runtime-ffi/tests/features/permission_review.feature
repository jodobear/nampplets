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

  Scenario: User changes apply beside unchanged host policy
    Given host policy manages the "theme" permission
    When the caller allows only "identity" against the current review
    Then the changed-domain permission update is applied
    And the "theme" permission remains controlled by host policy
    And the "identity" permission is allowed for the exact build

  Scenario: A host policy change makes an open review stale
    Given the caller has opened the current permission review
    When host policy takes over "theme" before the caller allows "identity"
    Then the FFI returns a typed stale-review refusal with the current review
    And the "identity" permission remains denied
