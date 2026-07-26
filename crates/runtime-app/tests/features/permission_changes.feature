Feature: Permission changes are revisioned Rust-owned intent

  Scenario: User choices apply beside unchanged managed policy
    Given an installed napplet mixes a managed setting with a user permission
    When the caller allows only the user permission against that review
    Then the user permission is allowed
    And the managed setting remains managed

  Scenario: An all-managed review accepts no fabricated decision
    Given an installed napplet has only a managed permission
    When the caller submits no permission changes
    Then the permission change is refused as empty
    And the managed setting remains managed

  Scenario: Managed policy changes make an open review stale
    Given an installed napplet has two user permissions under review
    When host policy takes control before the caller applies one permission
    Then the permission change is refused as stale
    And no user permission change was applied
