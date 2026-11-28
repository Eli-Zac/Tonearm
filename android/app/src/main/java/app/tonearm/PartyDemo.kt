package app.tonearm

import java.util.concurrent.CopyOnWriteArrayList

/**
 * A pretend Party, for trying the car's Party screens before the Party
 * server exists (see docs/party.md). It lives on this phone only: starting
 * one fills in a few made-up guests, and nobody can really join. The real
 * Party replaces this.
 */
object PartyDemo {
    data class Member(val name: String, val host: Boolean, val added: Int)

    const val NAME = "Your Party"
    const val CODE = "K7Q-M4P"

    @Volatile
    var active = false
        private set

    private val people = CopyOnWriteArrayList<Member>()
    val members: List<Member> get() = people.toList()

    /** Told after every change, for the car's button and pages. */
    private val watchers = CopyOnWriteArrayList<() -> Unit>()
    fun watch(change: () -> Unit) { watchers += change }
    fun unwatch(change: () -> Unit) { watchers -= change }
    private fun changed() = watchers.forEach { it() }

    fun start() {
        if (active) return
        people.clear()
        people += listOf(
            Member("You", host = true, added = 0),
            Member("Sam", host = false, added = 3),
            Member("Jordan", host = false, added = 1),
            Member("Riley", host = false, added = 0),
        )
        active = true
        changed()
    }

    fun end() {
        if (!active) return
        active = false
        people.clear()
        changed()
    }

    fun remove(name: String) {
        if (people.removeIf { it.name == name && !it.host }) changed()
    }
}
