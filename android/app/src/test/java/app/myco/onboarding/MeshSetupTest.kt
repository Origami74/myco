package app.myco.onboarding

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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

    // --- progress bar ---

    @Test
    fun theFirstCardIsStepTwoWithInstallDone() {
        assertEquals(2, MeshSetup.stepNumber(SetupStep.EnableMesh))
        assertEquals(
            listOf(SegmentState.Done, SegmentState.Active, SegmentState.Pending),
            MeshSetup.segments(SetupStep.EnableMesh, nearbyRefused = false),
        )
    }

    @Test
    fun aNearbyRefusalIsAmberOnItsOwnCardAndAfter() {
        assertEquals(2, MeshSetup.stepNumber(SetupStep.NearbyRefused))
        assertEquals(
            listOf(SegmentState.Done, SegmentState.Problem, SegmentState.Pending),
            MeshSetup.segments(SetupStep.NearbyRefused, nearbyRefused = true),
        )
        // Carried on past it: still amber while the VPN step runs.
        assertEquals(
            listOf(SegmentState.Done, SegmentState.Problem, SegmentState.Active),
            MeshSetup.segments(SetupStep.AskingVpn, nearbyRefused = true),
        )
    }

    @Test
    fun theVpnCardsAreStepThreeWithConnectionAmber() {
        for (step in listOf(SetupStep.VpnRefused, SetupStep.AlwaysOnVpn)) {
            assertEquals(3, MeshSetup.stepNumber(step))
            assertEquals(
                listOf(SegmentState.Done, SegmentState.Done, SegmentState.Problem),
                MeshSetup.segments(step, nearbyRefused = false),
            )
        }
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
        assertEquals(SetupNotice.AllSet, MeshSetup.notice(meshOn = true, nearbyGranted = true, declined = false))
        assertEquals(SetupNotice.NearbyMissing, MeshSetup.notice(meshOn = true, nearbyGranted = false, declined = false))
        assertEquals(SetupNotice.MeshOff, MeshSetup.notice(meshOn = false, nearbyGranted = true, declined = false))
        assertEquals(SetupNotice.Declined, MeshSetup.notice(meshOn = false, nearbyGranted = false, declined = true))
    }
}
