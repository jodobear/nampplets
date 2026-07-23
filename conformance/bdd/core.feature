Feature: Existing NIP-5D napplet compatibility

  @compat @legacy
  Scenario: Run an existing conformant napplet without modification
    Given the runtime is pinned to a compatibility baseline
    And a published napplet passes that baseline's conformance suite
    And all of its required domains are available
    When the user installs and launches the napplet
    Then the runtime executes only the verified artifact bytes plus the runtime-owned compatibility prelude
    And the napplet receives its expected window.napplet domains before its scripts run
    And no source or build change is required

  @compat
  Scenario: Reject a napplet with an unavailable required domain before execution
    Given a verified napplet requires the "ble" domain
    And the current platform does not advertise "ble"
    When the user attempts to launch it
    Then the runtime does not execute the napplet
    And the native UI identifies the missing domain

  @compat @forward
  Scenario: Ignore an unknown message type
    Given a mapped napplet session
    When the napplet emits a well-formed envelope with an unrecognized type
    Then the runtime silently ignores the envelope at the protocol boundary
    And the session remains healthy

Feature: Artifact integrity

  @security @artifact
  Scenario: Reject a blob whose bytes do not match its path hash
    Given a signed manifest references a blob hash
    And the blob source returns different bytes
    When the runtime resolves the artifact
    Then installation fails before execution
    And no returned bytes enter the executable cache

  @security @artifact
  Scenario: Reject an aggregate mismatch
    Given every path blob matches its individual hash
    But the manifest x tag does not match the recomputed aggregate
    When the runtime verifies the manifest
    Then installation fails before execution

Feature: WebView trust boundary

  @security @bridge
  Scenario: Drop a message from an unmapped source window
    Given one mapped napplet iframe and one unmapped iframe
    When the unmapped iframe emits a valid privileged envelope
    Then the trusted shell drops it
    And the native runtime receives no provider call

  @security @network
  Scenario: Block direct network access from a napplet
    Given a launched napplet
    When it attempts fetch and WebSocket access to an external host
    Then the browser denies both attempts
    And no provider grant is created

  @security @keys
  Scenario: Never expose ambient signing capability
    Given a launched napplet
    Then window.nostr is absent
    And no key or signer object is reachable from the napplet

Feature: Principal and permission isolation

  @security @storage
  Scenario: Isolate storage between builds
    Given two verified builds share a publisher and dTag but have different aggregate hashes
    And build A writes a storage key
    When build B reads the same key
    Then build B receives no value

  @security @update
  Scenario: Do not silently inherit sensitive grants on update
    Given build A has a persistent upload grant
    And build B is a verified update with a different aggregate hash
    When build B is installed
    Then build B does not receive the upload grant until the user explicitly approves it

  @security @revoke
  Scenario: Revoking a capability stops active non-durable work
    Given a napplet has an active resource stream
    When the user revokes the resource capability
    Then the stream is cancelled
    And future resource requests are denied

Feature: Surface mounting and state

  @surface
  Scenario: A descriptor-less napplet remains legacy
    Given a verified napplet has no surface descriptor
    When it launches
    Then it runs in the legacy profile
    And no surface domain is injected

  @surface @state
  Scenario: Mount a renderer with an initial snapshot
    Given a surface renderer declares a compatible feed input
    And the host has an active feed binding at revision 7
    When the component reports surface.ready
    Then the runtime sends a feed snapshot at revision 7
    And the component can render without opening a relay capability

  @surface @state
  Scenario: Recover from a missing delta
    Given a component has input revision 10
    When it receives a delta from revision 11 to revision 12
    Then it does not apply the delta
    And it requests resynchronization
    And the runtime sends the latest authoritative snapshot

  @surface @policy
  Scenario: Renderer profile cannot escalate into hybrid
    Given a component is mounted in renderer profile without outbox
    When it emits an outbox request
    Then no outbox provider is invoked
    And the request is denied or ignored according to the compatibility contract

Feature: Renderer replacement

  @surface @composition @nmp
  Scenario: Replace a feed renderer without restarting demand
    Given a workspace feed slot owns one NMP-backed binding
    And renderer A is mounted on that slot
    When the user replaces renderer A with renderer B
    Then the binding and NMP observation remain the same logical instances
    And renderer B receives the latest snapshot
    And renderer A is unmounted after renderer B is ready

  @surface @composition
  Scenario: Native and web renderers observe the same binding
    Given a native counter and a WebView feed consume one binding
    When the binding receives a new canonical event
    Then both consumers update from the same binding revision

Feature: Typed actions

  @surface @actions
  Scenario: Route a declared profile-open action
    Given a feed surface declares the profile.open action
    And the host has a preferred profile handler
    When the surface emits a valid profile.open payload
    Then the native action router opens the preferred handler
    And the action is recorded against the originating principal

  @surface @actions @security
  Scenario: Refuse an undeclared action
    Given a surface did not declare system.exec
    When it emits a system.exec action
    Then the runtime refuses it before any handler runs

Feature: Durable publication

  @nmp @write @surface
  Scenario: Publish from a component through native approval
    Given a composer emits a valid publish-draft action
    When the user approves the exact draft and account in native UI
    Then the runtime submits an NMP write intent
    And the pending event becomes visible through ordinary canonical queries

  @nmp @write @lifecycle
  Scenario: Publication survives component destruction
    Given NMP has accepted a durable write from a composer
    When the composer WebView is destroyed
    Then NMP retains the write obligation
    And another surface can observe its receipt and pending row

  @nmp @write @identity
  Scenario: Account switch cannot retarget an accepted write
    Given a write was accepted under account A
    When the user switches the active account to B
    Then the write remains bound to account A

Feature: Restart and offline behavior

  @offline
  Scenario: Launch an installed component while offline
    Given the artifact is verified and cached
    And NMP has cached rows for the binding
    And the network is unavailable
    When the user opens the workspace
    Then the component launches from local artifact bytes
    And cached rows render
    And evidence does not claim global completeness

  @restart @nmp
  Scenario: Restore a workspace and reattach a receipt
    Given a workspace contains a surface slot and a pending durable receipt
    When the application is terminated and restarted
    Then the workspace is restored
    And the runtime reattaches to the NMP receipt
    And the surface receives the latest receipt state

Feature: Resource limits and failure isolation

  @security @limits
  Scenario: One napplet exceeds its message-rate limit
    Given two healthy napplet sessions
    When session A floods envelopes beyond its finite quota
    Then session A is throttled or terminated with an observable reason
    And session B remains responsive

  @surface @backpressure
  Scenario: A slow surface converges without an unbounded queue
    Given a surface stops consuming state temporarily
    And the binding advances many times
    When the surface resumes
    Then it receives the newest correct snapshot or composed transition
    And memory does not grow with every skipped revision

  @crash
  Scenario: WebView crash does not stop native state
    Given a surface consumes an NMP-backed binding
    When its WebView process crashes
    Then the binding and NMP observation remain valid according to workspace ownership
    And the native host offers a bounded reload
