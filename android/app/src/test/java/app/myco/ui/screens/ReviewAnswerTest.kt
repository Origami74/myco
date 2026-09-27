package app.myco.ui.screens

import app.myco.core.NappletReview
import org.junit.Assert.assertEquals
import org.junit.Test

class ReviewAnswerTest {

    private fun review(
        installed: Boolean = false,
        ready: Boolean = false,
        unreviewed: List<String> = emptyList(),
    ) = NappletReview(
        pointer = "naddr1example",
        loading = false,
        installed = installed,
        ready = ready,
        unreviewed = unreviewed,
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

    @Test
    fun anInstalledAppThatIsNotHereIsDownloadedAgain() {
        assertEquals(ReviewAnswer.DownloadAgain, reviewAnswer(review(installed = true, ready = false)))
    }
}
