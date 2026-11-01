package app.myco.onboarding

/**
 * Where the setup popup is.
 *
 * The popup has up to four segments — Install Myco (always done), Nearby
 * devices, Connection and Name — and these are the states it can be in across
 * the last three. Only the two explain cards, the three refusal cards and
 * [Name] ask the user anything; the `Asking…` and [Connecting] states are what
 * sits behind Android's own prompts. Each Android prompt comes after a card
 * that says what it is for, and only on that card's button: the happy path is
 * "Allow nearby devices", Android's nearby prompt, then "Use this name".
 *
 * The mesh itself is on from the first launch: it needs no permission of its
 * own (Myco's traffic has its own stack in the core, no VPN). The popup only
 * asks for what the radios need. The Connection steps — the VPN — are in a run
 * only when the VPN is wanted: when other apps are allowed on the mesh (see
 * `MeshSharing`).
 *
 * A run opens on the first card that has something to ask ([MeshSetup.firstStep]),
 * never behind an Android prompt: a prompt is only ever launched by a tap on
 * one of the popup's buttons (see [MeshSetup.systemPromptUp]).
 */
enum class SetupStep {
    /**
     * "Nearby devices": what Android is about to ask for, before it asks. Its
     * "Allow nearby devices" launches the nearby prompt; "Not now" carries on without it,
     * as "Continue" on a refusal would. Shown only when a prompt would follow.
     */
    ExplainNearby,

    /** Android's nearby-devices prompt is up. */
    AskingNearby,

    /** The nearby permissions were refused: what won't work, Try again / Continue. */
    NearbyRefused,

    /**
     * "Other apps on the mesh": what the VPN Android is about to ask for is, before
     * it asks. Its "Allow VPN" launches the consent prompt; "Not now" skips the
     * VPN like a refusal would. Shown only when a prompt would follow.
     */
    ExplainVpn,

    /** Android's VPN consent prompt is up. */
    AskingVpn,

    /** Consent given; waiting for the tunnel to come up (or fail). */
    Connecting,

    /** VPN consent refused: "Other apps can’t use the mesh". */
    VpnRefused,

    /** Another app's always-on VPN holds the slot: "Another VPN is always on". */
    AlwaysOnVpn,

    /** The device name, edited in place on the card. Always last. */
    Name,
}

/** What opened this run of the popup. */
enum class SetupEntry {
    /** The app starting on a fresh install (see [MeshSetup.launchEntry]). */
    Launch,

    /** The app starting after an upgrade, with the mesh on and something it needs missing. */
    Upgrade,

    /** The user switched the mesh on (Settings, the status pill) with something missing. */
    MeshSwitch,

    /** A Fix in Settings (Permissions, the VPN warning card) or a radio toggle. */
    Fix,
}

/** A segment of the progress bar. */
enum class Segment { Install, Nearby, Connection, Name }

/** How one segment of the progress bar draws. */
enum class SegmentState { Done, Active, Problem, Pending }

/**
 * Which segments this run of the popup has. [mesh] covers Nearby devices and,
 * when [connection], the VPN; [name] is the name step. A fresh install has
 * both; the mesh switch reopens it with only the mesh steps once the name is
 * chosen; an upgrade whose mesh already works but never chose a name gets only
 * the name. [connection] is whether the VPN is wanted at all (other apps
 * allowed on the mesh); without it the mesh steps end after Nearby devices.
 */
data class SetupPlan(
    val mesh: Boolean,
    val name: Boolean,
    val entry: SetupEntry = SetupEntry.Launch,
    val connection: Boolean = true,
) {
    val segments: List<Segment> = buildList {
        add(Segment.Install)
        if (mesh) add(Segment.Nearby)
        if (mesh && connection) add(Segment.Connection)
        if (name) add(Segment.Name)
    }
}

/**
 * How the mesh steps of this run went, for drawing the segments behind the
 * one on screen.
 *
 * @param nearbyRefused carried on past a nearby refusal.
 * @param connectionSkipped "Not now" / "Continue without" on the VPN.
 */
data class MeshOutcome(
    val nearbyRefused: Boolean = false,
    val connectionSkipped: Boolean = false,
)

/** What to do about the popup when the app starts. */
enum class LaunchDecision {
    /** Show the popup on its first card with something to ask. */
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

    /** Mesh off. */
    MeshOff,
}

/**
 * The decisions behind the mesh setup popup, kept free of Android so they can
 * be unit-tested. The Activity owns the side effects (`SystemAsker` the prompts); this
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
     * A device whose admin set `DISALLOW_CONFIG_VPN` bounces it the same way
     * and gets the same card; its advice (turn the other VPN off) won't apply
     * there, but no prompt could have succeeded either.
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
     * @param vpnPrepared the VPN consent is Myco's — or the VPN isn't wanted,
     *   so there is none to ask for.
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
    fun launchPlan(
        mesh: LaunchDecision,
        nameChosen: Boolean,
        entry: SetupEntry = SetupEntry.Launch,
        connection: Boolean = true,
    ): SetupPlan? {
        val plan = SetupPlan(
            mesh = mesh == LaunchDecision.Show,
            name = !nameChosen,
            entry = entry,
            connection = connection,
        )
        return if (plan.mesh || plan.name) plan else null
    }

    /**
     * Whether a launch-time popup is a first run ([SetupEntry.Launch]) or an
     * upgrade ([SetupEntry.Upgrade]). The intro plays once, before anything
     * else, so a launch that hasn't seen it is a fresh install. So is one
     * whose first-run popup was left unfinished ([firstRunUnfinished]: the
     * app was closed mid-setup, after the intro). Anything else already ran
     * a build of Myco, mesh and all.
     */
    fun launchEntry(introSeen: Boolean, firstRunUnfinished: Boolean): SetupEntry =
        if (!introSeen || firstRunUnfinished) SetupEntry.Launch else SetupEntry.Upgrade

    /**
     * The mesh steps opened from outside the popup — the mesh switch, a
     * Settings fix, a radio toggle — with the name step only if it was never
     * answered. Opens on its first card with something to ask ([firstStep]).
     */
    fun reopenPlan(entry: SetupEntry, nameChosen: Boolean, connection: Boolean = true): SetupPlan =
        SetupPlan(mesh = true, name = !nameChosen, entry = entry, connection = connection)

    /**
     * The card a run of the popup opens on: the first one with something to
     * ask. The "Nearby devices" card when the nearby group is missing, then
     * the VPN card when this run wants the VPN and it isn't Myco's, then the
     * name. Null when nothing is left to ask. Never an `Asking…` step, so
     * opening the popup never puts an Android prompt up by itself.
     *
     * @param vpnPrepared the VPN consent is Myco's, or it isn't wanted.
     */
    fun firstStep(plan: SetupPlan, nearbyGranted: Boolean, vpnPrepared: Boolean): SetupStep? = when {
        plan.mesh && !nearbyGranted -> SetupStep.ExplainNearby
        plan.mesh && plan.connection && !vpnPrepared -> SetupStep.ExplainVpn
        plan.name -> SetupStep.Name
        else -> null
    }

    /**
     * Whether [step] is one that sits behind an Android prompt. The Activity
     * enters these only from a button tap on the popup, which launches the
     * prompt in the same call.
     */
    fun systemPromptUp(step: SetupStep?): Boolean =
        step == SetupStep.AskingNearby || step == SetupStep.AskingVpn

    /**
     * Where to go once the mesh steps are over, whichever way they ended: the
     * name step if this run has one, otherwise null, which closes the popup.
     */
    fun afterMesh(plan: SetupPlan): SetupStep? = if (plan.name) SetupStep.Name else null

    /**
     * Where to go once the nearby step is behind (granted, or carried on past
     * a refusal), in a run with the Connection steps: the card that explains
     * the VPN before Android asks for it — or, with the consent already Myco's
     * and so no prompt to come, straight to bringing the tunnel up. Never an
     * `Asking…` step: the VPN prompt waits for a tap on the card ([vpnStep]).
     * A run without them ([SetupPlan.connection]) goes to [afterMesh] instead.
     */
    fun afterNearby(vpnPrepared: Boolean): SetupStep =
        if (vpnPrepared) SetupStep.Connecting else SetupStep.ExplainVpn

    /**
     * The step a tap that asks for the VPN ("Allow VPN" on the connection card,
     * "Try again") lands on: Android's consent prompt, or, with the consent
     * already Myco's, the wait for the tunnel.
     */
    fun vpnStep(vpnPrepared: Boolean): SetupStep =
        if (vpnPrepared) SetupStep.Connecting else SetupStep.AskingVpn

    /**
     * "Not now" on the connection card or a VPN refusal, "Continue without" on
     * the always-on card: the Connection segment is marked skipped (other apps
     * stay off the mesh; Myco's own mesh carries on), then [afterMesh].
     */
    fun skipConnection(outcome: MeshOutcome): MeshOutcome = outcome.copy(connectionSkipped = true)

    /** The segment [step] belongs to. */
    fun segmentOf(step: SetupStep): Segment = when (step) {
        SetupStep.ExplainNearby, SetupStep.AskingNearby,
        SetupStep.NearbyRefused -> Segment.Nearby
        SetupStep.ExplainVpn, SetupStep.AskingVpn, SetupStep.Connecting,
        SetupStep.VpnRefused, SetupStep.AlwaysOnVpn -> Segment.Connection
        SetupStep.Name -> Segment.Name
    }

    /** "Step N of [SetupPlan.segments].size": [step]'s place in this run. */
    fun stepNumber(step: SetupStep, plan: SetupPlan): Int =
        plan.segments.indexOf(segmentOf(step)) + 1

    /**
     * How each of [plan]'s segments draws while [step] is on screen. Segments
     * behind it say how they went — done, or amber for a refusal the user
     * carried on past — and the ones ahead are pending, so the count stays the
     * same on every path.
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
    fun notice(meshOn: Boolean, nearbyGranted: Boolean): SetupNotice = when {
        !meshOn -> SetupNotice.MeshOff
        !nearbyGranted -> SetupNotice.NearbyMissing
        else -> SetupNotice.AllSet
    }
}
