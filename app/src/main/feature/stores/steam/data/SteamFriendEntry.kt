package com.winlator.cmod.feature.stores.steam.data

import com.winlator.cmod.feature.stores.steam.enums.EPersonaState

data class SteamFriendEntry(
    val steamId: Long,
    val name: String,
    val state: EPersonaState,
    val gameAppId: Int = 0,
    val gameName: String = "",
    val avatarHash: String = "",
    val connectString: String = "",
    val lobbyId: Long = 0L,
    val gameServerIp: Long = 0L,
    val gameServerPort: Int = 0,
) {
    // Mirrors Steam's own join precedence: connect command line, then lobby, then game server.
    // connectString is friend-controlled and reaches a Windows command line, hence the filter.
    val joinArgs: String = when {
        connectString.isNotBlank() -> connectString.trim().filter { it >= ' ' && it != '"' }
        lobbyId != 0L -> "+connect_lobby ${java.lang.Long.toUnsignedString(lobbyId)}"
        gameServerIp != 0L && gameServerPort > 0 -> "+connect ${ipv4(gameServerIp)}:$gameServerPort"
        else -> ""
    }

    val isJoinable: Boolean
        get() = isPlayingGame && gameAppId > 0 && joinArgs.isNotEmpty()

    val isOnline: Boolean
        get() = state.code() in 1..6

    val isPlayingGame: Boolean
        get() = isOnline && (gameAppId > 0 || gameName.isNotBlank())

    val avatarUrl: String?
        get() = avatarHash.takeIf { it.isNotBlank() }
            ?.let { "https://avatars.akamai.steamstatic.com/${it}_full.jpg" }

    // Game artwork for the app the friend is playing (Steam apps only).
    val gameCapsuleUrl: String?
        get() = gameAppId.takeIf { it > 0 }
            ?.let { "https://cdn.cloudflare.steamstatic.com/steam/apps/$it/capsule_231x87.jpg" }

    val gameHeaderUrl: String?
        get() = gameAppId.takeIf { it > 0 }
            ?.let { "https://cdn.cloudflare.steamstatic.com/steam/apps/$it/header.jpg" }
}

// Steam carries game_server_ip as a host-order uint32 — high byte is the first octet.
private fun ipv4(ip: Long): String =
    "${(ip shr 24) and 0xFF}.${(ip shr 16) and 0xFF}.${(ip shr 8) and 0xFF}.${ip and 0xFF}"
