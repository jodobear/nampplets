Feature: Durable receipt outcome is independent from observation lifecycle

  NMP owns the durable write result. The runtime FFI classifies only its
  canonical bounded state and exposes observation lifecycle on a separate axis.

  Scenario Outline: Canonical durable outcomes remain exhaustive
    Given a canonical "<fixture>" durable receipt state
    When the runtime FFI projects the receipt while observation is active
    Then the durable outcome is "<outcome>"
    And the observation lifecycle is "observing"
    And the exact canonical state remains available as raw evidence

    Examples:
      | fixture    | outcome     |
      | in-progress | in-progress |
      | delivered   | delivered   |
      | partial     | partial     |
      | exhausted   | exhausted   |
      | ambiguous   | ambiguous   |
      | refused     | refused     |
      | failed      | failed      |
      | cancelled   | cancelled   |
      | conflict    | conflict    |

  Scenario Outline: Unusable canonical evidence never becomes success
    Given a "<fixture>" durable receipt projection
    When the runtime FFI projects the receipt while observation is active
    Then the durable outcome is "unavailable"
    And the exact canonical state remains available as raw evidence

    Examples:
      | fixture   |
      | missing   |
      | malformed |
      | unknown   |
      | oversized |

  Scenario: Observation close does not weaken a delivered outcome
    Given a canonical "delivered" durable receipt state
    When the native observation closes after receiving that state
    Then the durable outcome is "delivered"
    And the observation lifecycle is "closed"
    And the exact canonical state remains available as raw evidence

  Scenario: Restart reattachment replays the same durable outcome
    Given a canonical "partial" durable receipt state
    When the same canonical state is replayed after receipt reattachment
    Then the durable outcome is "partial"
    And the observation lifecycle is "observing"
    And reattachment preserves the last durable outcome and evidence
