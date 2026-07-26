Feature: Bounded Rust performance evidence remains honest

  Scenario: Cold and warm Rust runs remain separate
    Given real cold and warm ResourceTracker evidence
    Then the two runs retain distinct state and comparison identity
    And their observed comparison is refused with a state mismatch

  Scenario: Raw samples reproduce percentiles and exact population variance
    Given ordered raw integer samples with durations 1, 2, and 3 nanoseconds
    When the Rust producer summarizes the samples
    Then nearest-rank percentiles and exact population variance are reproduced

  Scenario: Capacity refusal remains semantic and distinct from deadline
    Given a measured ResourceTracker capacity sibling
    Then SessionCapacity remains a semantic refusal and no deadline is recorded

  Scenario: Missing confidence remains not_evaluated
    Given two comparable artifacts without a ratified confidence method
    Then confidence remains not_evaluated with no_ratified_method

  Scenario: A diagnostic run cannot claim ratification
    Given a validator-accepted diagnostic result
    When the diagnostic result attempts to add a ratification claim
    Then the authoritative validator refuses the claim
