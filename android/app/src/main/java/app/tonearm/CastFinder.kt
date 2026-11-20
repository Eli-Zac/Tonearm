package app.tonearm

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import java.net.Inet4Address
import java.net.InetAddress
import java.util.concurrent.Executors
import uniffi.tonearm_android.Tonearm as Core

/**
 * Finds Cast devices on the network for the core. Android's own network
 * service discovery does the looking (apps are not given multicast the way
 * the desktop's finder uses it); each device found is handed to the core
 * with its address and TXT record, and when one goes the core is told.
 * Runs while the app is on screen.
 */
class CastFinder(context: Context, private val core: Core) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    /** Calls into the core, and Android's callbacks, run here, in order. */
    private val worker = Executors.newSingleThreadExecutor()
    private var listener: NsdManager.DiscoveryListener? = null
    /** Services with a live watch on them, by name (Android 14 and up). */
    private val watching = HashMap<String, NsdManager.ServiceInfoCallback>()
    /** Services waiting their turn to resolve: Android takes one at a time. */
    private val waiting = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    @Synchronized
    fun start() {
        if (listener != null) return
        val found = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                // Tried again on the next start; nothing else to do here.
                synchronized(this@CastFinder) { listener = null }
            }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onServiceFound(service: NsdServiceInfo) = resolve(service)
            override fun onServiceLost(service: NsdServiceInfo) = lost(service.serviceName)
        }
        listener = found
        try {
            nsd.discoverServices("_googlecast._tcp", NsdManager.PROTOCOL_DNS_SD, found)
        } catch (e: Exception) {
            listener = null
        }
    }

    @Synchronized
    fun stop() {
        val found = listener ?: return
        listener = null
        try {
            nsd.stopServiceDiscovery(found)
        } catch (e: Exception) {
        }
        if (Build.VERSION.SDK_INT >= 34) {
            watching.values.forEach { runCatching { nsd.unregisterServiceInfoCallback(it) } }
        }
        watching.clear()
        waiting.clear()
    }

    private fun resolve(service: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= 34) watch(service) else queue(service)
    }

    // ---------------------------------------------------- Android 14 and up

    @Synchronized
    private fun watch(service: NsdServiceInfo) {
        val name = service.serviceName
        if (watching.containsKey(name)) return
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                synchronized(this@CastFinder) { watching.remove(name) }
            }
            override fun onServiceUpdated(info: NsdServiceInfo) = seen(name, info, addresses(info))
            override fun onServiceLost() = lost(name)
            override fun onServiceInfoCallbackUnregistered() {}
        }
        watching[name] = callback
        try {
            nsd.registerServiceInfoCallback(service, worker, callback)
        } catch (e: Exception) {
            watching.remove(name)
        }
    }

    private fun addresses(info: NsdServiceInfo): List<InetAddress> =
        if (Build.VERSION.SDK_INT >= 34) info.hostAddresses else listOfNotNull(info.hostLegacy())

    @Suppress("DEPRECATION")
    private fun NsdServiceInfo.hostLegacy(): InetAddress? = host

    // ------------------------------------------------------ below Android 14

    @Synchronized
    private fun queue(service: NsdServiceInfo) {
        waiting.add(service)
        next()
    }

    @Synchronized
    private fun next() {
        if (resolving) return
        val service = waiting.removeFirstOrNull() ?: return
        resolving = true
        val done = {
            synchronized(this) { resolving = false }
            next()
        }
        @Suppress("DEPRECATION")
        try {
            nsd.resolveService(service, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) = done()
                override fun onServiceResolved(info: NsdServiceInfo) {
                    seen(service.serviceName, info, addresses(info))
                    done()
                }
            })
        } catch (e: Exception) {
            done()
        }
    }

    // ------------------------------------------------------------ to the core

    private fun seen(name: String, info: NsdServiceInfo, hosts: List<InetAddress>) {
        val address = (hosts.firstOrNull { it is Inet4Address } ?: hosts.firstOrNull())?.hostAddress ?: return
        val keys = ArrayList<String>()
        val values = ArrayList<String>()
        for ((key, value) in info.attributes) {
            keys.add(key)
            values.add(value?.toString(Charsets.UTF_8) ?: "")
        }
        val port = info.port.coerceIn(0, 65535).toUShort()
        worker.execute { runCatching { core.castSeen(name, keys, values, address, port) } }
    }

    private fun lost(name: String) {
        synchronized(this) {
            val callback = watching.remove(name)
            if (callback != null && Build.VERSION.SDK_INT >= 34) {
                runCatching { nsd.unregisterServiceInfoCallback(callback) }
            }
        }
        worker.execute { runCatching { core.castLost(name) } }
    }
}
