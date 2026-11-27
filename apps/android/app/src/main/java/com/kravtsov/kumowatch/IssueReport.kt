package com.kravtsov.kumowatch

import java.net.URLEncoder

data class Diagnostics(val version: String, val os: String, val log: String)

object IssueReport {
    const val NEW_ISSUE = "https://github.com/IgorKravtsov/better-kumo-app/issues/new"
    const val MAX_URL = 7000

    fun url(title: String, description: String, phone: String, computer: String, log: String?): String {
        val heading = title.trim().let { t ->
            // GitHub's title limit; never cut an emoji's surrogate pair in half.
            var n = minOf(256, t.length)
            if (n in 1 until t.length && t[n - 1].isHighSurrogate()) n--
            t.take(n)
        }.ifEmpty { "Problem: " }
        val text = description.trim()
        var lines = log?.lines()?.dropLastWhile { it.isBlank() }.orEmpty()
        while (lines.isNotEmpty()) {
            val url = build(heading, text, phone, computer, lines)
            if (url.length <= MAX_URL) return url
            lines = lines.drop(1)
        }
        if (build(heading, text, phone, computer, null).length <= MAX_URL) {
            return build(heading, text, phone, computer, null)
        }
        var low = 0
        var high = text.length
        while (low < high) {
            val mid = (low + high + 1) / 2
            if (build(heading, text.take(mid) + "…", phone, computer, null).length <= MAX_URL) low = mid else high = mid - 1
        }
        if (low > 0 && text[low - 1].isHighSurrogate()) low--
        return build(heading, text.take(low) + "…", phone, computer, null)
    }

    private fun build(title: String, description: String, phone: String, computer: String, lines: List<String>?): String {
        val body = buildString {
            append("**What happened**\n")
            if (description.isNotEmpty()) append(description).append("\n")
            append("<!-- Add screenshots here: tap \"Paste, drop, or click to add files\" below. -->\n\n")
            append("**Environment**\n- Phone: $phone\n- Computer: $computer\n")
            if (lines != null) {
                append("\n<details><summary>Computer log (last ${lines.size} lines, names and ids removed)</summary>\n\n")
                append("```text\n").append(lines.joinToString("\n")).append("\n```\n</details>\n")
            }
        }
        return "$NEW_ISSUE?title=${encode(title)}&body=${encode(body)}"
    }

    private fun encode(value: String): String = URLEncoder.encode(value, "UTF-8").replace("+", "%20")
}
