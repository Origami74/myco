package app.myco.ui.screens

import app.myco.core.NappletReview
import org.junit.Assert.assertEquals
import org.junit.Test

class ReviewAnswerTest {

    private fun review(
        installed: Boolean = false,
        ready: Boolean = false,
        unreviewed: List<String> = emptyList(),
        updateAvailable: Boolean = false,
        alreadyInstalled: Boolean = false,
    ) = NappletReview(
        pointer = "naddr1example",
        loading = false,
        installed = installed,
        ready = ready,
        unreviewed = unreviewed,
        updateAvailable = updateAvailable,
        alreadyInstalled = alreadyInstalled,
        title = "AppStore",
        description = "",
        requires = listOf("outbox", "theme"),
        grants = listOf("outbox", "theme"),
        error = "",
    )

    @Test
    fun aNewAppIsAdded() {
        assertEquals(ReviewAnswer.Add, reviewAnswer(review()))
    }

    /** The review an open hands its window when an update asks for more. */
    @Test
    fun anUpdateAskingForMoreIsAllowed() {
        assertEquals(
            ReviewAnswer.Allow,
            reviewAnswer(review(installed = true, ready = true, unreviewed = listOf("outbox"))),
        )
    }

    @Test
    fun anUpdateAskingForMoreIsAllowedEvenWhenNotHere() {
        assertEquals(
            ReviewAnswer.Allow,
            reviewAnswer(review(installed = true, ready = false, unreviewed = listOf("outbox"))),
        )
    }

    @Test
    fun anInstalledAppWithNothingNewIsNotAddedAgain() {
        assertEquals(ReviewAnswer.AlreadyInstalled, reviewAnswer(review(installed = true, ready = true)))
    }

    /** The core's answer wins: installed, here and current. */
    @Test
    fun theCoresAlreadyInstalledIsShownAsSuch() {
        assertEquals(
            ReviewAnswer.AlreadyInstalled,
            reviewAnswer(review(installed = true, ready = true, alreadyInstalled = true)),
        )
    }

    /** A newer version asking for nothing new is an update, never "Already installed". */
    @Test
    fun aNewerVersionIsAnUpdate() {
        assertEquals(
            ReviewAnswer.Update,
            reviewAnswer(review(installed = true, ready = true, updateAvailable = true)),
        )
    }

    /** A newer version asking for more is still asked about first. */
    @Test
    fun aNewerVersionAskingForMoreIsAllowed() {
        assertEquals(
            ReviewAnswer.Allow,
            reviewAnswer(
                review(installed = true, ready = true, updateAvailable = true, unreviewed = listOf("outbox")),
            ),
        )
    }

    @Test
    fun anInstalledAppThatIsNotHereIsDownloadedAgain() {
        assertEquals(ReviewAnswer.DownloadAgain, reviewAnswer(review(installed = true, ready = false)))
    }
}
