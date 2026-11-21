package app.tonearm

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.tonearm_android.Connections
import uniffi.tonearm_android.ItemKind
import uniffi.tonearm_android.Profile
import uniffi.tonearm_android.TopArtist
import uniffi.tonearm_android.Tonearm as Core

/** The profile page's route id, in a tab's stack like any page. */
const val PROFILE_ROUTE = "tonearm:profile"

/** The desktop's Artists tab in search, by its place among the tabs. */
private const val ARTISTS_TAB = 3u

/**
 * The profile page, as the desktop's: who is signed in, what the history on
 * this phone says (counts, top artists, songs played lately) and the
 * connections. It is read again each time the page opens, since leaving it
 * takes it out of the composition. Skeletons hold the sizes of what comes,
 * so nothing moves when it lands.
 */
@Composable
fun ProfileScreen(
    core: Core,
    actions: Actions,
    onOpenSettings: (String) -> Unit,
    onBack: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val account = rememberAccount(core)
    val loaded by produceState<Pair<Profile, Connections>?>(null, Prefs.version) {
        value = withContext(Dispatchers.IO) { core.profile() to core.connections() }
    }
    val profile = loaded?.first
    val connections = loaded?.second

    // An artist the history has no page for is looked up, as a search on
    // the Artists tab, and the first one found opens.
    fun openArtist(artist: TopArtist) {
        val id = artist.id
        if (id != null) {
            actions.open(id, null, artist.name)
            return
        }
        scope.launch {
            val found = withContext(Dispatchers.IO) {
                runCatching {
                    core.searchTab(artist.name, ARTISTS_TAB).shelves.flatMap { it.items }
                        .firstOrNull { it.kind == ItemKind.ARTIST && it.browseId != null }
                }.getOrNull()
            }
            if (found != null) actions.open(found.browseId!!, found.params, found.title)
            else actions.notify("Couldn't find ${artist.name}")
        }
    }

    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).statusBarsPadding()
                .padding(top = 56.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(28.dp),
        ) {
            ProfileHeader(account, core, onOpenSettings)

            // What the history holds, in three tiles.
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Title("On this phone", size = 22.sp, weight = FontWeight.ExtraBold)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    val songs = profile?.songs?.toInt()
                    StatTile(songs, if (songs == 1) "Song played" else "Songs played", Modifier.weight(1f))
                    val artists = profile?.artistCount?.toInt()
                    StatTile(artists, if (artists == 1) "Artist" else "Artists", Modifier.weight(1f))
                    StatTile(profile?.opened?.toInt(), "Albums and playlists", Modifier.weight(1f))
                }
                if (profile != null && profile.songs == 0u && profile.opened == 0u) {
                    Dim("Nothing yet. What you play and open shows up here. It stays on this phone.", 14.sp, lines = 3)
                }
            }

            // Artists played most.
            if (profile == null || profile.artists.isNotEmpty()) {
                Column(Modifier.padding(horizontal = 16.dp)) {
                    if (profile == null) {
                        Box(Modifier.height(28.dp), contentAlignment = Alignment.CenterStart) {
                            Bone(Modifier.width(170.dp).height(22.dp), RoundedCornerShape(7.dp))
                        }
                    } else {
                        Title("Your top artists", size = 22.sp, weight = FontWeight.ExtraBold)
                    }
                    Spacer(Modifier.height(10.dp))
                    if (profile == null) {
                        repeat(5) { ArtistRankSkeleton(it) }
                    } else {
                        profile.artists.forEachIndexed { i, artist -> ArtistRank(i + 1, artist) { openArtist(artist) } }
                    }
                }
            }

            // Songs played lately: the cards of Home's shelves, and a tap
            // does what a tap does there.
            if (profile == null || profile.played.isNotEmpty()) {
                Column {
                    Box(Modifier.padding(start = 16.dp, end = 16.dp, bottom = 12.dp).height(28.dp), contentAlignment = Alignment.CenterStart) {
                        if (profile == null) Bone(Modifier.width(150.dp).height(22.dp), RoundedCornerShape(7.dp))
                        else Title("Played lately", size = 22.sp, weight = FontWeight.ExtraBold)
                    }
                    if (profile == null) {
                        Row(
                            Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                            horizontalArrangement = Arrangement.spacedBy(14.dp),
                        ) {
                            repeat(4) { CardSkeleton(it) }
                        }
                    } else {
                        LazyRow(
                            contentPadding = PaddingValues(horizontal = 16.dp),
                            horizontalArrangement = Arrangement.spacedBy(14.dp),
                        ) {
                            items(profile.played) { ItemCard(it, 150.dp, false, actions) }
                        }
                    }
                }
            }

            // Last.fm and Spotify, opening their settings page.
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Title("Connections", size = 22.sp, weight = FontWeight.ExtraBold)
                SubpageRow(
                    title = "Last.fm",
                    subtitle = connections?.let { if (it.lastfmUser.isNotEmpty()) "Scrobbling as ${it.lastfmUser}" else "Not linked" },
                    logo = R.drawable.logo_lastfm,
                    brand = brandLastfm,
                ) { onOpenSettings(SettingsPages.lastfm) }
                SubpageRow(
                    title = "Spotify",
                    subtitle = connections?.let {
                        when {
                            !it.spotifyLinked -> "Not linked"
                            it.canvasLinked -> "Track details and Canvas"
                            else -> "Track details"
                        }
                    },
                    logo = R.drawable.logo_spotify,
                    brand = brandSpotify,
                ) { onOpenSettings(SettingsPages.spotify) }
            }
        }
        StatusFade()
        BackButton(onBack)
    }
}

/** The picture, who it is, and the buttons for the account. */
@Composable
private fun ProfileHeader(account: AccountState, core: Core, onOpenSettings: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    Column(Modifier.padding(horizontal = 16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            AccountPhoto(account, 96.dp)
            Column(Modifier.weight(1f).padding(start = 16.dp)) {
                Dim("Profile", 13.sp)
                when (account) {
                    AccountState.Loading -> {
                        Box(Modifier.height(35.dp), contentAlignment = Alignment.CenterStart) {
                            Bone(Modifier.fillMaxWidth(0.6f).height(26.dp))
                        }
                        Box(Modifier.height(18.dp), contentAlignment = Alignment.CenterStart) {
                            Bone(Modifier.fillMaxWidth(0.4f).height(12.dp))
                        }
                    }
                    is AccountState.SignedIn -> {
                        Title(account.account.name, size = 28.sp, weight = FontWeight.ExtraBold)
                        if (account.account.handle.isNotEmpty()) Dim(account.account.handle, 14.sp)
                    }
                    AccountState.SignedOut -> {
                        Title("Not signed in", size = 28.sp, weight = FontWeight.ExtraBold)
                        Dim("Sign in to YouTube Music for your library, playlists and mixes.", 14.sp, lines = 3)
                    }
                }
            }
        }
        // The buttons get a row of their own under the picture: beside
        // it, a phone is too narrow for both.
        Spacer(Modifier.height(16.dp))
        Box(Modifier.height(36.dp)) {
            when (account) {
                AccountState.Loading -> Bone(Modifier.width(150.dp).height(36.dp), RoundedCornerShape(18.dp))
                is AccountState.SignedIn -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PillButton("Account settings") { onOpenSettings(SettingsPages.youtube) }
                    PillButton("Sign out", danger = true) {
                        scope.launch {
                            withContext(Dispatchers.IO) { core.signOut() }
                            Prefs.changed()
                        }
                    }
                }
                AccountState.SignedOut -> PillButton("Sign in", primary = true) { onOpenSettings(SettingsPages.signIn) }
            }
        }
    }
}

/** A count and what it counts; a bone stands in for the number until it is known. */
@Composable
private fun StatTile(value: Int?, label: String, modifier: Modifier = Modifier) {
    Column(
        // Numbers on one line across the tiles: from the top, so a label
        // wrapping to two lines (narrow phones) only uses room below.
        modifier.height(84.dp).clip(RoundedCornerShape(12.dp)).background(Color(0x0AFFFFFF))
            .padding(horizontal = 14.dp).padding(top = 12.dp),
    ) {
        Box(Modifier.height(35.dp), contentAlignment = Alignment.CenterStart) {
            if (value == null) Bone(Modifier.width(44.dp).height(26.dp))
            else Title(value.toString(), size = 28.sp, weight = FontWeight.ExtraBold)
        }
        Dim(label, 13.sp, lines = 2)
    }
}

/** One of the top artists: their place, a round picture holder and the name. */
@Composable
private fun ArtistRank(rank: Int, artist: TopArtist, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(10.dp)).clickable(onClick = onClick),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            rank.toString(), modifier = Modifier.width(22.dp), textAlign = TextAlign.End,
            color = if (rank <= 3) Tonearm.accent else Tonearm.dim,
            fontFamily = Tonearm.font, fontWeight = FontWeight.Bold, fontSize = 16.sp, maxLines = 1,
        )
        Box(
            Modifier.padding(start = 12.dp).size(32.dp).clip(CircleShape).background(Color(0x14FFFFFF)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.artist, size = 14.sp, color = Tonearm.dim)
        }
        Title(artist.name, size = 15.sp, modifier = Modifier.weight(1f).padding(start = 12.dp))
    }
}

/** An artist's row before they are known: the same 48dp, a bone each. */
@Composable
private fun ArtistRankSkeleton(seed: Int) {
    Row(Modifier.fillMaxWidth().height(48.dp), verticalAlignment = Alignment.CenterVertically) {
        Spacer(Modifier.width(22.dp))
        Bone(Modifier.padding(start = 12.dp).size(32.dp), CircleShape)
        Bone(Modifier.padding(start = 12.dp).fillMaxWidth(0.55f - (seed * 7 % 4) * 0.07f).height(13.dp))
    }
}
