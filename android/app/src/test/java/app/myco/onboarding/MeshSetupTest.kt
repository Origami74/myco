package app.myco.onboarding

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MeshSetupTest {

    // --- when the popup shows at launch ---

    @Test
    fun aFreshInstallSeesThePopup() {
        // Nothing stored, nothing granted.
        assertEquals(
            LaunchDecision.Show,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = null, nearbyGranted = false, vpnPrepared = false),
        )
    }

    @Test
    fun anUpgradeWithAWorkingMeshIsNotNagged() {
        assertEquals(
            LaunchDecision.MarkDone,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = true, nearbyGranted = true, vpnPrepared = true),
        )
        // The pref was never written on an install that kept the default.
        assertEquals(
            LaunchDecision.MarkDone,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = null, nearbyGranted = true, vpnPrepared = true),
        )
    }

    @Test
    fun anUpgradeWithMeshOnButSomethingMissingSeesIt() {
        assertEquals(
            LaunchDecision.Show,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = true, nearbyGranted = false, vpnPrepared = true),
        )
        assertEquals(
            LaunchDecision.Show,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = true, nearbyGranted = true, vpnPrepared = false),
        )
    }

    @Test
    fun anUpgradeWithMeshSwitchedOffIsADecision() {
        assertEquals(
            LaunchDecision.MarkDone,
            MeshSetup.atLaunch(onboardingDone = false, meshPref = false, nearbyGranted = false, vpnPrepared = false),
        )
    }

    @Test
    fun doneMeansLaunchNeverAsksAgain() {
        for (mesh in listOf(null, true, false)) {
            assertEquals(
                LaunchDecision.None,
                MeshSetup.atLaunch(onboardingDone = true, meshPref = mesh, nearbyGranted = false, vpnPrepared = false),
            )
        }
    }

    @Test
    fun switchingMeshOnSkipsThePopupOnlyWithNothingMissing() {
        assertFalse(MeshSetup.meshOnNeedsSetup(nearbyGranted = true, vpnPrepared = true))
        assertTrue(MeshSetup.meshOnNeedsSetup(nearbyGranted = false, vpnPrepared = true))
        assertTrue(MeshSetup.meshOnNeedsSetup(nearbyGranted = true, vpnPrepared = false))
    }

    // --- which steps a run has ---

    private val full = SetupPlan(mesh = true, name = true)
    private val meshOnly = SetupPlan(mesh = true, name = false)
    private val nameOnly = SetupPlan(mesh = false, name = true)
    private val D = SegmentState.Done
    private val A = SegmentState.Active
    private val P = SegmentState.Pending
    private val X = SegmentState.Problem

    @Test
    fun aFreshInstallHasAllFourSegments() {
        val plan = MeshSetup.launchPlan(LaunchDecision.Show, nameChosen = false)
        assertEquals(full, plan)
        assertEquals(listOf(Segment.Install, Segment.Nearby, Segment.Connection, Segment.Name), full.segments)
        // No confirm card: it opens on the first thing to ask.
        assertEquals(SetupStep.ExplainNearby, MeshSetup.firstStep(full, nearbyGranted = false, vpnPrepared = false))
    }

    @Test
    fun withoutTheVpnWantedThereIsNoConnectionSegment() {
        val plan = MeshSetup.launchPlan(LaunchDecision.Show, nameChosen = false, connection = false)!!
        assertEquals(listOf(Segment.Install, Segment.Nearby, Segment.Name), plan.segments)
        assertEquals(3, MeshSetup.stepNumber(SetupStep.Name, plan))
        assertEquals(listOf(D, D, A), MeshSetup.segments(SetupStep.Name, plan, MeshOutcome()))
        val reopened = MeshSetup.reopenPlan(SetupEntry.Fix, nameChosen = true, connection = false)
        assertEquals(listOf(Segment.Install, Segment.Nearby), reopened.segments)
    }

    @Test
    fun anUpgradeWithAWorkingMeshButNoNameAsksOnlyTheName() {
        val plan = MeshSetup.launchPlan(LaunchDecision.MarkDone, nameChosen = false)
        assertEquals(nameOnly, plan)
        assertEquals(SetupStep.Name, MeshSetup.firstStep(nameOnly, nearbyGranted = false, vpnPrepared = false))
        assertEquals(2, MeshSetup.stepNumber(SetupStep.Name, nameOnly))
        assertEquals(2, nameOnly.segments.size)
        assertEquals(listOf(D, A), MeshSetup.segments(SetupStep.Name, nameOnly, MeshOutcome()))
    }

    @Test
    fun nothingToAskOpensNothing() {
        assertNull(MeshSetup.launchPlan(LaunchDecision.MarkDone, nameChosen = true))
        assertNull(MeshSetup.launchPlan(LaunchDecision.None, nameChosen = true))
        assertEquals(meshOnly, MeshSetup.launchPlan(LaunchDecision.Show, nameChosen = true))
    }

    @Test
    fun theNameComesAfterTheMeshStepsWhicheverWayTheyEnded() {
        assertEquals(SetupStep.Name, MeshSetup.afterMesh(full))
        // The mesh switch reopening it after the name was chosen: no name step.
        assertNull(MeshSetup.afterMesh(meshOnly))
    }

    // --- a run opens on its first card with something to ask ---

    @Test
    fun theFirstCardIsWhateverIsMissingFirst() {
        // Nearby first, then the VPN (when this run wants it), then the name.
        assertEquals(SetupStep.ExplainNearby, MeshSetup.firstStep(full, nearbyGranted = false, vpnPrepared = true))
        assertEquals(SetupStep.ExplainVpn, MeshSetup.firstStep(full, nearbyGranted = true, vpnPrepared = false))
        assertEquals(SetupStep.Name, MeshSetup.firstStep(full, nearbyGranted = true, vpnPrepared = true))
        // A run without the VPN never opens on its card.
        val noVpn = SetupPlan(mesh = true, name = false, connection = false)
        assertNull(MeshSetup.firstStep(noVpn, nearbyGranted = true, vpnPrepared = false))
        assertEquals(SetupStep.ExplainNearby, MeshSetup.firstStep(noVpn, nearbyGranted = false, vpnPrepared = false))
        // Nothing missing and no name to ask: nothing to show.
        assertNull(MeshSetup.firstStep(meshOnly, nearbyGranted = true, vpnPrepared = true))
    }

    @Test
    fun meshSwitchedOnLaterOpensOnTheNearbyCardWithNoRequestPending() {
        assertTrue(MeshSetup.meshOnNeedsSetup(nearbyGranted = false, vpnPrepared = true))
        val plan = MeshSetup.reopenPlan(SetupEntry.MeshSwitch, nameChosen = true)
        assertEquals(SetupPlan(mesh = true, name = false, entry = SetupEntry.MeshSwitch), plan)
        val first = MeshSetup.firstStep(plan, nearbyGranted = false, vpnPrepared = true)
        assertEquals(SetupStep.ExplainNearby, first)
        assertFalse(MeshSetup.systemPromptUp(first))
        assertEquals(listOf(D, A, P), MeshSetup.segments(first!!, plan, MeshOutcome()))
        assertNull(MeshSetup.afterMesh(plan))
    }

    @Test
    fun noRunOpensBehindAnAndroidPrompt() {
        val plans = listOf(full, meshOnly, nameOnly) +
            SetupEntry.entries.flatMap { e -> listOf(true, false).map { MeshSetup.reopenPlan(e, it) } }
        for (plan in plans) {
            for (nearby in listOf(true, false)) {
                for (vpn in listOf(true, false)) {
                    assertFalse(MeshSetup.systemPromptUp(MeshSetup.firstStep(plan, nearby, vpn)))
                }
            }
        }
        assertTrue(MeshSetup.systemPromptUp(SetupStep.AskingNearby))
        assertTrue(MeshSetup.systemPromptUp(SetupStep.AskingVpn))
        assertFalse(MeshSetup.systemPromptUp(null))
    }

    // --- first run or upgrade ---

    @Test
    fun aLaunchBeforeTheIntroIsAFirstRun() {
        assertEquals(SetupEntry.Launch, MeshSetup.launchEntry(introSeen = false, firstRunUnfinished = false))
    }

    @Test
    fun aFirstRunClosedMidSetupIsStillAFirstRun() {
        // The intro is behind it, but the popup it showed never finished.
        assertEquals(SetupEntry.Launch, MeshSetup.launchEntry(introSeen = true, firstRunUnfinished = true))
    }

    @Test
    fun aLaunchAfterTheIntroIsAnUpgrade() {
        assertEquals(SetupEntry.Upgrade, MeshSetup.launchEntry(introSeen = true, firstRunUnfinished = false))
    }

    // --- nearby devices are explained before Android asks for them ---

    @Test
    fun theNearbyCardIsStepTwoWithNoPromptPending() {
        val next = SetupStep.ExplainNearby
        assertFalse(MeshSetup.systemPromptUp(next))
        // Step 2 of 4, Nearby active.
        assertEquals(2, MeshSetup.stepNumber(next, full))
        assertEquals(listOf(D, A, P, P), MeshSetup.segments(next, full, MeshOutcome()))
    }

    // --- the VPN is explained before Android asks for it ---

    @Test
    fun afterTheNearbyResultComesTheConnectionCardWithNoPromptPending() {
        val next = MeshSetup.afterNearby(vpnPrepared = false)
        assertEquals(SetupStep.ExplainVpn, next)
        assertFalse(MeshSetup.systemPromptUp(next))
        // Step 3 of 4, Connection active (a card, not a problem).
        assertEquals(3, MeshSetup.stepNumber(next, full))
        assertEquals(listOf(D, D, A, P), MeshSetup.segments(next, full, MeshOutcome()))
        // Carried on past a nearby refusal: the same card, Nearby amber behind it.
        assertEquals(
            listOf(D, X, A, P),
            MeshSetup.segments(next, full, MeshOutcome(nearbyRefused = true)),
        )
    }

    @Test
    fun continueOnTheConnectionCardAsksForTheVpn() {
        val next = MeshSetup.vpnStep(vpnPrepared = false)
        assertEquals(SetupStep.AskingVpn, next)
        assertTrue(MeshSetup.systemPromptUp(next))
    }

    @Test
    fun notNowOnTheConnectionCardSkipsTheVpnLikeARefusal() {
        val outcome = MeshSetup.skipConnection(MeshOutcome())
        assertEquals(MeshOutcome(connectionSkipped = true), outcome)
        assertEquals(SetupStep.Name, MeshSetup.afterMesh(full))
        assertEquals(listOf(D, D, X, A), MeshSetup.segments(SetupStep.Name, full, outcome))
        assertNull(MeshSetup.afterMesh(meshOnly))
    }

    @Test
    fun withTheVpnAlreadyMycosTheCardIsSkipped() {
        assertEquals(SetupStep.Connecting, MeshSetup.afterNearby(vpnPrepared = true))
        assertEquals(SetupStep.Connecting, MeshSetup.vpnStep(vpnPrepared = true))
        assertFalse(MeshSetup.systemPromptUp(SetupStep.Connecting))
    }

    // --- progress bar ---

    @Test
    fun nearbyThenNameCountsOneToFour() {
        assertEquals(2, MeshSetup.stepNumber(SetupStep.ExplainNearby, full))
        assertEquals(3, MeshSetup.stepNumber(SetupStep.Connecting, full))
        assertEquals(listOf(D, D, A, P), MeshSetup.segments(SetupStep.Connecting, full, MeshOutcome()))
        // The Name card is the last step: 4 of 4.
        assertEquals(4, MeshSetup.stepNumber(SetupStep.Name, full))
        assertEquals(listOf(D, D, D, A), MeshSetup.segments(SetupStep.Name, full, MeshOutcome()))
    }

    @Test
    fun theMeshSwitchRunCountsToThree() {
        assertEquals(2, MeshSetup.stepNumber(SetupStep.AskingNearby, meshOnly))
        assertEquals(3, MeshSetup.stepNumber(SetupStep.VpnRefused, meshOnly))
        assertEquals(3, meshOnly.segments.size)
    }

    @Test
    fun aNearbyRefusalIsAmberOnItsOwnCardAndAfter() {
        assertEquals(listOf(D, X, P, P), MeshSetup.segments(SetupStep.NearbyRefused, full, MeshOutcome()))
        val refused = MeshOutcome(nearbyRefused = true)
        assertEquals(listOf(D, X, A, P), MeshSetup.segments(SetupStep.AskingVpn, full, refused))
        assertEquals(listOf(D, X, D, A), MeshSetup.segments(SetupStep.Name, full, refused))
    }

    @Test
    fun theVpnCardsAreStepThreeWithConnectionAmber() {
        for (step in listOf(SetupStep.VpnRefused, SetupStep.AlwaysOnVpn)) {
            assertEquals(3, MeshSetup.stepNumber(step, full))
            assertEquals(listOf(D, D, X, P), MeshSetup.segments(step, full, MeshOutcome()))
        }
        // "Not now" / "Continue without", then the name.
        assertEquals(
            listOf(D, D, X, A),
            MeshSetup.segments(SetupStep.Name, full, MeshOutcome(connectionSkipped = true)),
        )
    }

    // --- the always-on heuristic ---

    @Test
    fun consentGivenIsGrantedHoweverFast() {
        assertEquals(VpnConsent.Granted, MeshSetup.classifyConsent(resultOk = true, elapsedMs = 50))
        assertEquals(VpnConsent.Granted, MeshSetup.classifyConsent(resultOk = true, elapsedMs = 5_000))
    }

    @Test
    fun anInstantCancelMeansAnotherAlwaysOnVpn() {
        assertEquals(VpnConsent.BlockedByAlwaysOn, MeshSetup.classifyConsent(resultOk = false, elapsedMs = 80))
        assertEquals(
            VpnConsent.BlockedByAlwaysOn,
            MeshSetup.classifyConsent(resultOk = false, elapsedMs = MeshSetup.FAST_ANSWER_MS - 1),
        )
    }

    @Test
    fun aCancelSomeoneCouldHaveTappedIsARefusal() {
        assertEquals(
            VpnConsent.Refused,
            MeshSetup.classifyConsent(resultOk = false, elapsedMs = MeshSetup.FAST_ANSWER_MS),
        )
        assertEquals(VpnConsent.Refused, MeshSetup.classifyConsent(resultOk = false, elapsedMs = 3_000))
    }

    @Test
    fun aNegativeElapsedIsNotReadAsInstant() {
        // A launch time restored wrongly must not invent an always-on VPN.
        assertEquals(VpnConsent.Refused, MeshSetup.classifyConsent(resultOk = false, elapsedMs = -1))
    }

    @Test
    fun establishFailingAfterConsentMeansBlocked() {
        assertEquals(TunnelWait.Blocked, MeshSetup.tunnel(isUp = false, establishFailed = true, waitedMs = 300))
    }

    @Test
    fun theTunnelWaitEndsUpOrGivesUp() {
        assertEquals(TunnelWait.Up, MeshSetup.tunnel(isUp = true, establishFailed = false, waitedMs = 0))
        // Up wins over an older failure: the retry worked.
        assertEquals(TunnelWait.Up, MeshSetup.tunnel(isUp = true, establishFailed = true, waitedMs = 0))
        assertEquals(TunnelWait.Waiting, MeshSetup.tunnel(isUp = false, establishFailed = false, waitedMs = 2_000))
        assertEquals(
            TunnelWait.GaveUp,
            MeshSetup.tunnel(isUp = false, establishFailed = false, waitedMs = MeshSetup.TUNNEL_WAIT_MS),
        )
    }

    @Test
    fun nearbyRefusedForGoodNeedsAllThreeSigns() {
        assertTrue(MeshSetup.refusedForGood(allGranted = false, elapsedMs = 40, anyRationale = false))
        // Someone tapped "Don't allow": slow enough to be a person.
        assertFalse(MeshSetup.refusedForGood(allGranted = false, elapsedMs = 2_000, anyRationale = false))
        // Android would still explain and ask again.
        assertFalse(MeshSetup.refusedForGood(allGranted = false, elapsedMs = 40, anyRationale = true))
        assertFalse(MeshSetup.refusedForGood(allGranted = true, elapsedMs = 40, anyRationale = false))
    }

    // --- how it ends ---

    @Test
    fun theSnackbarSaysHowItEnded() {
        assertEquals(SetupNotice.AllSet, MeshSetup.notice(meshOn = true, nearbyGranted = true))
        assertEquals(SetupNotice.NearbyMissing, MeshSetup.notice(meshOn = true, nearbyGranted = false))
        assertEquals(SetupNotice.MeshOff, MeshSetup.notice(meshOn = false, nearbyGranted = true))
    }
}
