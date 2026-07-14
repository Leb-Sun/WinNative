package com.winlator.cmod.feature.stores.steam.utils

/** Parses Steam-style launch options: KEY=VALUE before %command% become env vars; args after become game args. */
object SteamLaunchOptions {
    private val ENV_KEY = Regex("[A-Za-z_][A-Za-z0-9_]*")
    // No-%command% autodetect is UPPERCASE-only so bare game args (-dx11) and lowercase
    // key=value args stay game args, while real env vars (DXVK_HUD, WINEDLLOVERRIDES) are caught.
    private val ENV_KEY_UPPER = Regex("[A-Z_][A-Z0-9_]*")
    private const val COMMAND = "%command%"

    data class Parsed(val env: LinkedHashMap<String, String>, val gameArgs: String)

    @JvmStatic
    fun parse(execArgs: String?): Parsed {
        val env = LinkedHashMap<String, String>()
        val raw = execArgs?.trim().orEmpty()
        if (raw.isEmpty()) return Parsed(env, "")
        val idx = raw.indexOf(COMMAND)
        if (idx < 0) return Parsed(env, raw.substring(consumeLeadingEnv(raw, env)).trim())
        val before = raw.substring(0, idx).trim()
        val after = raw.substring(idx + COMMAND.length).trim()
        for (token in tokenize(before)) {
            val eq = token.indexOf('=')
            if (eq <= 0) continue
            val key = token.substring(0, eq)
            if (ENV_KEY.matches(key)) env[key] = token.substring(eq + 1)
        }
        return Parsed(env, after)
    }

    // Without %command%, a leading run of UPPERCASE KEY=VALUE tokens is env vars (shell-style
    // prefix); returns the offset where the first non-env token begins (game args from there,
    // kept verbatim so quotes survive). == raw.length when every token was an env var.
    private fun consumeLeadingEnv(raw: String, env: LinkedHashMap<String, String>): Int {
        var i = 0
        val n = raw.length
        while (i < n) {
            while (i < n && raw[i].isWhitespace()) i++
            if (i >= n) return n
            val start = i
            val sb = StringBuilder()
            var quote: Char? = null
            while (i < n && (quote != null || !raw[i].isWhitespace())) {
                val c = raw[i]
                when {
                    quote != null -> if (c == quote) quote = null else sb.append(c)
                    c == '"' || c == '\'' -> quote = c
                    else -> sb.append(c)
                }
                i++
            }
            val token = sb.toString()
            val eq = token.indexOf('=')
            if (eq > 0 && ENV_KEY_UPPER.matches(token.substring(0, eq))) {
                env[token.substring(0, eq)] = token.substring(eq + 1)
            } else {
                return start
            }
        }
        return n
    }

    @JvmStatic
    fun gameArgs(execArgs: String?): String = parse(execArgs).gameArgs

    @JvmStatic
    fun parseEnvVars(execArgs: String?): Map<String, String> = parse(execArgs).env

    private fun tokenize(s: String): List<String> {
        val out = ArrayList<String>()
        val sb = StringBuilder()
        var quote: Char? = null
        for (c in s) {
            when {
                quote != null -> if (c == quote) quote = null else sb.append(c)
                c == '"' || c == '\'' -> quote = c
                c.isWhitespace() -> if (sb.isNotEmpty()) {
                    out.add(sb.toString()); sb.setLength(0)
                }
                else -> sb.append(c)
            }
        }
        if (sb.isNotEmpty()) out.add(sb.toString())
        return out
    }
}
