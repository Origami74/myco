package app.myco.onboarding

/**
 * Where the setup popup is.
 *
 * The popup has up to four segments — Install Myco (always done), Nearby
 * phones, Connection and Name — and these are the states it can be in across
 * the last three. Only [EnableMesh], the three refusal cards and [Name] ask the
 * user anything; the `Asking…` and [Connecting] states are what sits behind
 * Android's own prompts, so the happy path is "Yes", Android's nearby prompt,
 * Android's VPN prompt, then "Use this name".
 *
 * Every run that has mesh steps opens on [EnableMesh], whoever opened it: an
 * Android prompt is only ever launched by a tap on one of the popup's buttons,
 * never by the popup opening (see [MeshSetup.systemPromptUp]).
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

    /** The device name, edited in place on the card. Always last. */
    Name,
}

/**
 * What opened this run of the popup. It decides what the grey button on
 * "Enable mesh?" does.
 */
enum class SetupEntry {
    /** The app starting: a fresh install, or an upgrade with something missing. */
    Launch,

    /** The user switched the mesh on (Settings, the status pill) with something missing. */
    MeshSwitch,

    /** A Fix in Settings (Permissions, the VPN warning card) or a radio toggle. */
    Fix,
}

/** A segment of the progress bar. */
enum class Segment { Install, Nearby, Connection, Name }

/** How one segment of the progress bar draws. */
enum class SegmentState { Done, Active, Problem, Pending, Skipped }

/**
 * Which segments this run of the popup has. [mesh] covers Nearby phones and
 * Connection together; [name] is the name step. A fresh install has both; the
 * mesh switch reopens it with only the mesh steps once the name is chosen; an
 * upgrade whose mesh already works but never chose a name gets only the name.
 */
data class SetupPlan(
    val mesh: Boolean,
    val name: Boolean,
    val entry: SetupEntry = SetupEntry.Launch,
) {
    val segments: List<Segment> = buildList {
        add(Segment.Install)
        if (mesh) addAll(listOf(Segment.Nearby, Segment.Connection))
        if (name) add(Segment.Name)
    }
}

/**
 * How the mesh steps of this run went, for drawing the segments behind the
 * one on screen.
 *
 * @param declined "No thanks": Nearby and Connection were skipped.
 * @param nearbyRefused carried on past a nearby refusal.
 * @param connectionSkipped "Not now" / "Continue without" on the VPN.
 * @param dismissed "Not now" on "Enable mesh?" in a run the user opened
 *   later: nothing was asked and nothing changed, so it leaves no snackbar.
 */
data class MeshOutcome(
    val declined: Boolean = false,
    val nearbyRefused: Boolean = false,
    val connectionSkipped: Boolean = false,
    val dismissed: Boolean = false,
)

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

    /**
     * What this launch puts in the popup: the mesh steps when [atLaunch] says
     * [LaunchDecision.Show], the name step when no name was ever chosen.
     * Null when there is nothing to ask.
     */
    fun launchPlan(mesh: LaunchDecision, nameChosen: Boolean): SetupPlan? {
        val plan = SetupPlan(mesh = mesh == LaunchDecision.Show, name = !nameChosen)
        return if (plan.mesh || plan.name) plan else null
    }

    /**
     * The mesh steps opened from outside the popup — the mesh switch, a
     * Settings fix, a radio toggle — with the name step only if it was never
     * answered. Opens on "Enable mesh?" like every other run ([firstStep]).
     */
    fun reopenPlan(entry: SetupEntry, nameChosen: Boolean): SetupPlan =
        SetupPlan(mesh = true, name = !nameChosen, entry = entry)

    /**
     * The card a run of the popup opens on. With mesh steps that is always
     * the "Enable mesh?" confirmation — never an `Asking…` step, so opening
     * the popup never puts an Android prompt up by itself.
     */
    fun firstStep(plan: SetupPlan): SetupStep =
        if (plan.mesh) SetupStep.EnableMesh else SetupStep.Name

    /**
     * Whether [step] is one that sits behind an Android prompt. The Activity
     * enters these only from a button tap on the popup, which launches the
     * prompt in the same call.
     */
    fun systemPromptUp(step: SetupStep?): Boolean =
        step == SetupStep.AskingNearby || step == SetupStep.AskingVpn

    /**
     * Whether the grey button on "Enable mesh?" just closes the popup ("Not
     * now", nothing changed) rather than declining the mesh ("No thanks", on
     * to the name). Only the launch run declines; a run the user opened later
     * by switching the mesh on or tapping a Fix closes and leaves things be.
     */
    fun greyButtonCloses(plan: SetupPlan): Boolean = plan.entry != SetupEntry.Launch

    /**
     * Where to go once the mesh steps are over, whichever way they ended
     * ("No thanks" included — the name matters without mesh too): the name
     * step if this run has one, otherwise null, which closes the popup.
     */
    fun afterMesh(plan: SetupPlan): SetupStep? = if (plan.name) SetupStep.Name else null

    /** The segment [step] belongs to. */
    fun segmentOf(step: SetupStep): Segment = when (step) {
        SetupStep.EnableMesh, SetupStep.AskingNearby, SetupStep.NearbyRefused -> Segment.Nearby
        SetupStep.AskingVpn, SetupStep.Connecting, SetupStep.VpnRefused, SetupStep.AlwaysOnVpn ->
            Segment.Connection
        SetupStep.Name -> Segment.Name
    }

    /** "Step N of [SetupPlan.segments].size": [step]'s place in this run. */
    fun stepNumber(step: SetupStep, plan: SetupPlan): Int =
        plan.segments.indexOf(segmentOf(step)) + 1

    /**
     * How each of [plan]'s segments draws while [step] is on screen. Segments
     * behind it say how they went — done, amber for a refusal the user carried
     * on past, struck through for steps "No thanks" skipped — and the ones
     * ahead are pending, so the count stays the same on every path.
     */
    fun segments(step: SetupStep, plan: SetupPlan, outcome: MeshOutcome): List<SegmentState> {
        val current = segmentOf(step)
        val currentIndex = plan.segments.indexOf(current)
        val refusalCard = step == SetupStep.NearbyRefused ||
            step == SetupStep.VpnRefused || step == SetupStep.AlwaysOnVpn
        return plan.segments.mapIndexed { i, seg ->
            when {
                seg == Segment.Install -> SegmentState.Done
                i > currentIndex -> SegmentState.Pending
                i == currentIndex -> if (refusalCard) SegmentState.Problem else SegmentState.Active
                outcome.declined || outcome.dismissed -> SegmentState.Skipped
                seg == Segment.Nearby && outcome.nearbyRefused -> SegmentState.Problem
                seg == Segment.Connection && outcome.connectionSkipped -> SegmentState.Problem
                else -> SegmentState.Done
            }
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
