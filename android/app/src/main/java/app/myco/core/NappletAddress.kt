package app.myco.core

/**
 * Which napplet a pointer names — its author and `d` tag — whichever way the
 * pointer is spelled: an `naddr1…` (optionally `nostr:`-prefixed) or the
 * `<npub>:<dtag>` shorthand.
 *
 * The same napplet reaches Kotlin under both spellings (a scanned `naddr`, a
 * Library entry written from `<npub>:<dtag>`), so comparing pointers as
 * strings misses. Two pointers name the same napplet when their addresses are
 * equal. Checksums are not verified: every pointer here came through the core,
 * which already parsed it.
 */
data class NappletAddress(val author: String, val dTag: String?) {
    companion object {
        /** Null when [pointer] is neither spelling. */
        fun parse(pointer: String): NappletAddress? {
            val p = pointer.trim().removePrefix("nostr:")
            if (p.startsWith("naddr1")) return fromNaddr(p)
            val npub = p.substringBefore(':')
            val author = bech32Bytes(npub, "npub")?.takeIf { it.size == 32 } ?: return null
            val dTag = if (p.contains(':')) p.substringAfter(':') else null
            return NappletAddress(author.toHex(), dTag?.ifEmpty { null })
        }

        /** The address of an author npub and `d` tag, as a Library entry stores them. */
        fun of(authorNpub: String, dTag: String?): NappletAddress? =
            bech32Bytes(authorNpub, "npub")?.takeIf { it.size == 32 }
                ?.let { NappletAddress(it.toHex(), dTag?.ifEmpty { null }) }

        /** NIP-19 TLV: 0 = `d` identifier, 2 = author. */
        private fun fromNaddr(naddr: String): NappletAddress? {
            val bytes = bech32Bytes(naddr, "naddr") ?: return null
            var dTag: String? = null
            var author: ByteArray? = null
            var i = 0
            while (i + 2 <= bytes.size) {
                val type = bytes[i].toInt() and 0xff
                val len = bytes[i + 1].toInt() and 0xff
                if (i + 2 + len > bytes.size) return null
                val value = bytes.copyOfRange(i + 2, i + 2 + len)
                when (type) {
                    0 -> dTag = String(value, Charsets.UTF_8)
                    2 -> author = value
                }
                i += 2 + len
            }
            val a = author?.takeIf { it.size == 32 } ?: return null
            return NappletAddress(a.toHex(), dTag?.ifEmpty { null })
        }

        private const val CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"

        /** The data part of a bech32 string with prefix [hrp], checksum stripped. */
        private fun bech32Bytes(s: String, hrp: String): ByteArray? {
            val lower = s.lowercase()
            if (!lower.startsWith(hrp + "1")) return null
            val data = lower.substring(hrp.length + 1).map { CHARSET.indexOf(it) }
            if (data.size < 7 || data.any { it < 0 }) return null
            var acc = 0
            var bits = 0
            val out = ArrayList<Byte>(data.size * 5 / 8)
            for (v in data.subList(0, data.size - 6)) {
                acc = ((acc shl 5) or v) and 0xfff
                bits += 5
                while (bits >= 8) {
                    bits -= 8
                    out.add(((acc shr bits) and 0xff).toByte())
                }
            }
            return out.toByteArray()
        }

        private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
    }
}
