Feature: Provider push authority is fail-closed

  A provider's push sender must reject envelopes that spoof caller identity
  or target the wrong domain, and must fail closed the instant its
  capability is revoked.

  Background:
    Given an installed and running napplet with a live provider push sender

  Scenario: A push spoofing the principal is refused
    When the provider pushes "canary.state" with a spoofed principal field
    Then the push is refused with an authority-field error

  Scenario: A push to the wrong domain is refused
    When the provider pushes "other.state" with no payload
    Then the push is refused with a domain-mismatch error

  Scenario: Revoking the capability fails closed for further pushes
    Given the session has become ready
    When the capability is revoked
    Then a further push on that domain is refused as revoked
    And the provider observed exactly one revocation for that session
