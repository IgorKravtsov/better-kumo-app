package com.kravtsov.kumowatch

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.URLDecoder

class IssueReportTest {
    private val phone = "Better Kumo Android 0.2.17 (1), Google Pixel 9, Android 16"
    private val mac = "Better Kumo 0.2.17 on macOS 15.1 (aarch64)"

    private fun title(url: String): String =
        URLDecoder.decode(url.substringAfter("?title=").substringBefore("&body="), "UTF-8")

    private fun body(url: String): String =
        URLDecoder.decode(url.substringAfter("&body="), "UTF-8")

    private fun longLog() = (1..300).joinToString("\n") { "line $it with some relay output to make it long enough" }

    @Test
    fun emptyTitleFallsBackAndSpacesArePercentEncoded() {
        val url = IssueReport.url("  ", "Chat stopped", phone, "not paired", null)
        assertTrue(url.startsWith("${IssueReport.NEW_ISSUE}?title=Problem%3A%20&body="))
        assertFalse(url.contains('+'))
        assertFalse(url.contains(' '))
        assertEquals(
            "**What happened**\nChat stopped\n" +
                "<!-- Add screenshots here: tap \"Paste, drop, or click to add files\" below. -->\n\n" +
                "**Environment**\n- Phone: $phone\n- Computer: not paired\n",
            body(url),
        )
    }

    @Test
    fun titleAndLogWithLineCount() {
        val url = IssueReport.url("Nudges + knocks lost", "", phone, mac, "one\ntwo\nthree\n")
        assertEquals("Nudges + knocks lost", title(url))
        val body = body(url)
        assertTrue(body.startsWith("**What happened**\n<!-- Add screenshots"))
        assertTrue(body.contains("Computer log (last 3 lines, names and ids removed)"))
        assertTrue(body.contains("```text\none\ntwo\nthree\n```\n</details>"))
    }

    @Test
    fun dropsOldestLogLinesToFit() {
        val url = IssueReport.url("Title", "Short", phone, mac, longLog())
        assertTrue(url.length <= IssueReport.MAX_URL)
        val body = body(url)
        assertTrue(body.contains("line 300 with"))
        assertFalse(body.contains("line 1 with"))
        val kept = Regex("last (\\d+) lines").find(body)!!.groupValues[1].toInt()
        assertTrue(kept in 1 until 300)
        assertEquals(kept, body.substringAfter("```text\n").substringBefore("\n```").lines().size)
    }

    @Test
    fun cutsTheDescriptionOnlyAfterTheWholeLog() {
        val description = "word ".repeat(3000)
        val url = IssueReport.url("Title", description, phone, mac, longLog())
        assertTrue(url.length <= IssueReport.MAX_URL)
        assertTrue(url.length > IssueReport.MAX_URL - 20)
        val body = body(url)
        assertFalse(body.contains("<details>"))
        assertTrue(body.contains("word…\n<!-- Add screenshots"))
        assertTrue(body.contains("- Computer: $mac"))
    }

    @Test
    fun anEmojiAtTheTitleLimitStaysWhole() {
        val url = IssueReport.url("a".repeat(255) + "😀b", "x", phone, mac, null)
        assertEquals("a".repeat(255), title(url))
    }

    @Test
    fun hugeEnvironmentAndDraftStillFit() {
        val url = IssueReport.url("T", "é".repeat(20_000), "p".repeat(10_000), "c".repeat(10_000), longLog())
        assertTrue(url.length <= IssueReport.MAX_URL)
        assertFalse(body(url).contains("line 300"))
    }
}
