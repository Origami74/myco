package app.myco.onboarding

/**
 * Where the mesh setup popup is.
 *
 * The popup has three segments — Install Myco (always done), Nearby phones and
 * Connection — and these are the states it can be in across the last two. Only
 * [EnableMesh] and the three refusal cards ask the user anything; the `Asking…`
 * and [Connecting] states are what sits behind Android's own prompts, so the
 * happy path is three taps: "Yes", Android's nearby prompt, Android's VPN prompt.
 */
enum class SetupStep {
    /** "Enable mesh?" — the first card, and the only one on the happy path. */
    EnableMesh,

    /** Android's nearby-devices prompt is up. */
    AskingNearby,

    /** The nearby permissions were refused: what won't work, Try again / Continue. */
    NearbyRefused,

    /** Android's VPN consent prompt is up. */
    AskingVpn,

    /** Consent given; waiting for the tunnel to come up (or fail). */
    Connecting,

    /** VPN consent refused: "Mesh needs the VPN". */
    VpnRefused,

    /** Another app's always-on VPN holds the slot: "Another VPN is always on". */
    AlwaysOnVpn,
}

/** How one segment of the progress bar draws. */
enum class SegmentState { Done, Active, Problem, Pending }

/** What to do about the popup when the app starts. */
enum class LaunchDecision {
    /** Show the popup at "Enable mesh?". */
    Show,

    /** Nothing to ask: record setup as done without showing anything. */
    MarkDone,

    /** Setup was done before; leave it to Settings. */
    None,
}

/** What the VPN consent activity's result means. */
enum class VpnConsent { Granted, Refused, BlockedByAlwaysOn }

/** Where the wait for the tunnel after consent stands. */
enum class TunnelWait { Up, Blocked, Waiting, GaveUp }

/** The snackbar the popup leaves behind when it closes. */
enum class SetupNotice {
    /** Everything granted, tunnel up. */
    AllSet,

    /** Mesh on, but the nearby permissions were refused. */
    NearbyMissing,

    /** Mesh off because the VPN step was skipped or blocked. */
    MeshOff,

    /** Mesh off because the user said "No thanks". */
    Declined,
}

/**
 * The decisions behind the mesh setup popup, kept free of Android so they can
 * be unit-tested. The Activity owns the launchers and the side effects; this
 * only says what comes next.
 */
object MeshSetup {

    /**
     * A result that comes back faster than this was not a person's answer.
     *
     * Android's VPN consent activity (`VpnDialogs/ConfirmDialog`) finishes
     * straight away, with `RESULT_CANCELED` and no dialog, when another app is
     * set as the always-on VPN — Myco can't become the VPN then, and Android
     * gives no API to say so. A real "Cancel" tap needs the dialog drawn, read
     * and tapped, which takes well over this. The same holds for a permission
     * request Android answers without showing anything (refused for good).
     *
     * A heuristic, with known limits: a very slow device could take longer than
     * this to bounce the activity (we then show "Mesh needs the VPN", which is
     * still correct about the outcome, just not the cause), and nothing here can
     * name the app holding the slot — so the card says "another app's VPN".
     */
    const val FAST_ANSWER_MS = 500L

    /**
     * How long to wait after consent for the tunnel before closing the popup
     * anyway. The node can take a few seconds to publish its address
     * (`startMeshNow` retries for ten); past this the Settings warning card
     * owns the problem.
     */
    const val TUNNEL_WAIT_MS = 12_000L

    /**
     * Whether to show the popup at launch.
     *
     * - Done before → never again on its own; Settings › Permissions and the
     *   mesh switch reopen it.
     * - Not done, and the mesh was switched off on purpose ([meshPref] false,
     *   from a build before this popup) → that was a decision; don't ask.
     * - Not done, mesh on (or never set: a fresh install), and everything it
     *   needs is already granted → an upgrade with a working mesh. Don't nag;
     *   mark it done.
     * - Otherwise the mesh is on but something is missing, or nobody has
     *   decided yet → show it.
     *
     * @param meshPref the stored mesh switch, null when it was never written.
     */
    fun atLaunch(
        onboardingDone: Boolean,
        meshPref: Boolean?,
        nearbyGranted: Boolean,
        vpnPrepared: Boolean,
    ): LaunchDecision = when {
        onboardingDone -> LaunchDecision.None
        meshPref == false -> LaunchDecision.MarkDone
        nearbyGranted && vpnPrepared -> LaunchDecision.MarkDone
        else -> LaunchDecision.Show
    }

    /**
     * Whether switching the mesh on has to go through the popup. With nothing
     * missing it just switches on, with no popup to flash past.
     */
    fun meshOnNeedsSetup(nearbyGranted: Boolean, vpnPrepared: Boolean): Boolean =
        !(nearbyGranted && vpnPrepared)

    /** "Step N of 3": the Nearby phones steps are 2, the Connection steps 3. */
    fun stepNumber(step: SetupStep): Int = when (step) {
        SetupStep.EnableMesh, SetupStep.AskingNearby, SetupStep.NearbyRefused -> 2
        else -> 3
    }

    /**
     * The three segments — Install Myco, Nearby phones, Connection — for
     * [step]. [nearbyRefused] keeps the Nearby segment amber once the user
     * carried on past a refusal.
     */
    fun segments(step: SetupStep, nearbyRefused: Boolean): List<SegmentState> {
        val nearbyDone = if (nearbyRefused) SegmentState.Problem else SegmentState.Done
        return when (step) {
            SetupStep.EnableMesh, SetupStep.AskingNearby ->
                listOf(SegmentState.Done, SegmentState.Active, SegmentState.Pending)
            SetupStep.NearbyRefused ->
                listOf(SegmentState.Done, SegmentState.Problem, SegmentState.Pending)
            SetupStep.AskingVpn, SetupStep.Connecting ->
                listOf(SegmentState.Done, nearbyDone, SegmentState.Active)
            SetupStep.VpnRefused, SetupStep.AlwaysOnVpn ->
                listOf(SegmentState.Done, nearbyDone, SegmentState.Problem)
        }
    }

    /**
     * Read the VPN consent activity's result. See [FAST_ANSWER_MS] for why a
     * near-instant cancel means another app's always-on VPN.
     */
    fun classifyConsent(resultOk: Boolean, elapsedMs: Long): VpnConsent = when {
        resultOk -> VpnConsent.Granted
        elapsedMs in 0 until FAST_ANSWER_MS -> VpnConsent.BlockedByAlwaysOn
        else -> VpnConsent.Refused
    }

    /**
     * The second always-on signal: consent was given, but
     * `VpnService.Builder.establish()` then failed (returned null or threw).
     * With consent in hand, the usual cause is another VPN that holds the slot
     * — always-on with lockdown, or one that took it back in between.
     *
     * @param establishFailed `establish()` failed after the consent was given.
     */
    fun tunnel(isUp: Boolean, establishFailed: Boolean, waitedMs: Long): TunnelWait = when {
        isUp -> TunnelWait.Up
        establishFailed -> TunnelWait.Blocked
        waitedMs >= TUNNEL_WAIT_MS -> TunnelWait.GaveUp
        else -> TunnelWait.Waiting
    }

    /**
     * Whether Android refused the nearby permissions without asking: denied,
     * answered faster than anyone could tap, and no rationale on offer — the
     * "don't ask again" state. "Try again" then has to send the user to the
     * app's settings page, since another request would bounce the same way.
     */
    fun refusedForGood(allGranted: Boolean, elapsedMs: Long, anyRationale: Boolean): Boolean =
        !allGranted && !anyRationale && elapsedMs in 0 until FAST_ANSWER_MS

    /** The snackbar for how the popup ended. */
    fun notice(meshOn: Boolean, nearbyGranted: Boolean, declined: Boolean): SetupNotice = when {
        declined -> SetupNotice.Declined
        !meshOn -> SetupNotice.MeshOff
        !nearbyGranted -> SetupNotice.NearbyMissing
        else -> SetupNotice.AllSet
    }
}
