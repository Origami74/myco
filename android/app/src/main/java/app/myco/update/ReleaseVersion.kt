package app.myco.update

/** Only stable SemVer tags can produce an update notice. Compare numerically. */
data class ReleaseVersion(val major: Long, val minor: Long, val patch: Long) : Comparable<ReleaseVersion> {
    override fun compareTo(other: ReleaseVersion): Int =
        compareValuesBy(this, other, { it.major }, { it.minor }, { it.patch })

    companion object {
        private val pattern = Regex("^v?(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?$")
        fun parse(value: String): ReleaseVersion? {
            val match = pattern.matchEntire(value) ?: return null
            val parts = (1..3).map { match.groupValues[it].toLongOrNull() ?: return null }
            return ReleaseVersion(parts[0], parts[1], parts[2])
        }
    }
}

fun isNewerRelease(installed: String, tag: String): Boolean {
    val current = ReleaseVersion.parse(installed) ?: return false
    val candidate = ReleaseVersion.parse(tag) ?: return false
    return candidate > current
}

/** Wall-clock rollback must not permanently disable checks. */
fun updateCheckDue(now: Long, lastAttempt: Long, interval: Long): Boolean =
    lastAttempt == 0L || now < lastAttempt || now - lastAttempt >= interval
