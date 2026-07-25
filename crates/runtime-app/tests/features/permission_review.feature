Feature: Permission review is exact-build bounded

  A permission review reflects only the capabilities one specific installed
  exact build requested, and denying a required capability keeps that build
  from launching.

  Background:
    Given an installed napplet that requires the "canary" capability and optionally requests the "missing" capability

  Scenario: The review lists only this exact build's requested capabilities
    When the caller requests a permission review for the installed napplet
    Then the review is scoped to that exact build's principal
    And the review lists exactly 2 capabilities
    And the "canary" capability is available with ordinary sensitivity
    And the "missing" capability has unknown platform availability
    And the "missing" capability defaults to a denied decision

  Scenario: Denying a required capability blocks launch
    Given the caller has denied every requested capability in one batch
    When the caller attempts to launch the installed napplet
    Then no session is admitted
    And the most recent runtime error is a bridge refusal
