Feature: Live provider updates stay scoped to live delivery

  A provider update must not make unrelated installed-library, workspace,
  receipt, pending-write, session, binding, resource, activity, or error
  state appear changed.

  Background:
    Given an installed and running napplet with a live provider push sender
    And the session has become ready
    And the current producer section revisions have been recorded

  Scenario: A live update changes only delivery and event replay identity
    When the provider delivers a valid live canary state update
    Then only live delivery and event replay are marked changed
