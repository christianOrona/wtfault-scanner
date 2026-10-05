package com.wtfault.scanner

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.view.WindowManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/**
 * Bluetooth Classic (SPP) links to a paired ELM327, for the Rust core.
 *
 * The core's transport is blocking and polls; this side never blocks. Commands
 * are dispatched on the UI thread, so anything slow (connecting, writing) runs
 * on [io] and resolves later, and [read] only hands over what the reader
 * thread has already buffered. See `src/android.rs` for the other half.
 */
@SuppressLint("MissingPermission") // checked in hasPermission() before any call that needs it
@TauriPlugin
class BluetoothClassicPlugin(private val activity: Activity) : Plugin(activity) {

    /** The Serial Port Profile UUID every ELM327 clone answers on. */
    private val spp: UUID = UUID.fromString("00001101-0000-1000-8000-00805F9B34FB")

    private val links = ConcurrentHashMap<String, Link>()
    private val io = Executors.newSingleThreadExecutor()

    @InvokeArg
    class AddressArgs {
        lateinit var address: String
    }

    @InvokeArg
    class WriteArgs {
        lateinit var address: String
        var data: IntArray = IntArray(0)
    }

    @InvokeArg
    class ReadArgs {
        lateinit var address: String
        var max: Int = 4096
    }

    private fun adapter(): BluetoothAdapter? =
        (activity.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter

    /** Android 12 made connecting to a paired device a runtime permission. */
    private fun hasPermission(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S ||
            ContextCompat.checkSelfPermission(activity, Manifest.permission.BLUETOOTH_CONNECT) ==
            PackageManager.PERMISSION_GRANTED

    /**
     * Paired devices, or the reason there are none. A reason is always given:
     * an empty list with no explanation is indistinguishable from a bug.
     */
    @Command
    fun list(invoke: Invoke) {
        val ret = JSObject()
        val adapter = adapter()
        when {
            adapter == null -> ret.put("problem", "This device has no Bluetooth.")
            !hasPermission() -> {
                ActivityCompat.requestPermissions(
                    activity,
                    arrayOf(Manifest.permission.BLUETOOTH_CONNECT),
                    PERMISSION_REQUEST,
                )
                ret.put(
                    "problem",
                    "The app needs the Nearby devices permission to reach a paired adapter. " +
                        "Allow it, then press Refresh.",
                )
            }
            !adapter.isEnabled -> ret.put("problem", "Bluetooth is off. Turn it on, then press Refresh.")
            else -> {
                val devices = JSArray()
                for (device in adapter.bondedDevices) {
                    val d = JSObject()
                    d.put("name", device.name)
                    d.put("address", device.address)
                    devices.put(d)
                }
                if (devices.length() == 0) {
                    ret.put(
                        "problem",
                        "No paired Bluetooth devices. Pair the adapter in Android's Bluetooth " +
                            "settings first (the PIN is usually 1234 or 0000), then press Refresh.",
                    )
                }
                ret.put("devices", devices)
            }
        }
        invoke.resolve(ret)
    }

    @Command
    fun open(invoke: Invoke) {
        val args = invoke.parseArgs(AddressArgs::class.java)
        io.execute {
            try {
                val adapter = adapter() ?: throw IOException("this device has no Bluetooth")
                if (!hasPermission()) throw IOException("the Nearby devices permission is not granted")
                if (!adapter.isEnabled) throw IOException("Bluetooth is off")
                val device = adapter.getRemoteDevice(args.address)
                // Discovery slows a connect to a crawl. Cancelling it needs a
                // scan permission from Android 12 on, which the app never
                // asks for because it never starts discovery.
                if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) adapter.cancelDiscovery()
                links.remove(args.address)?.close()
                val link = Link(connect(device)) { keepAwakeWhileLinked() }
                links[args.address] = link
                link.start()
                keepAwakeWhileLinked()
                invoke.resolve(JSObject())
            } catch (e: Exception) {
                invoke.reject("could not connect to ${args.address}: ${e.message ?: e.javaClass.simpleName}")
            }
        }
    }

    /**
     * Try the ways an ELM327 clone is known to accept a connection, most
     * standard first. Plenty of clones refuse a secure socket, and a few only
     * answer on RFCOMM channel 1 without an SDP lookup at all.
     */
    private fun connect(device: BluetoothDevice): BluetoothSocket {
        val attempts: List<() -> BluetoothSocket> = listOf(
            { device.createRfcommSocketToServiceRecord(spp) },
            { device.createInsecureRfcommSocketToServiceRecord(spp) },
            {
                device.javaClass
                    .getMethod("createRfcommSocket", Int::class.javaPrimitiveType)
                    .invoke(device, 1) as BluetoothSocket
            },
        )
        var last: Exception? = null
        for (attempt in attempts) {
            var socket: BluetoothSocket? = null
            try {
                socket = attempt()
                socket.connect()
                return socket
            } catch (e: Exception) {
                last = e
                try { socket?.close() } catch (_: IOException) {}
            }
        }
        throw last ?: IOException("no connection method worked")
    }

    @Command
    fun write(invoke: Invoke) {
        val args = invoke.parseArgs(WriteArgs::class.java)
        val link = links[args.address]
        if (link == null) {
            invoke.reject("${args.address} is not open")
            return
        }
        val bytes = ByteArray(args.data.size) { args.data[it].toByte() }
        io.execute {
            try {
                link.socket.outputStream.write(bytes)
                link.socket.outputStream.flush()
                invoke.resolve(JSObject())
            } catch (e: IOException) {
                link.open = false
                invoke.reject("write to ${args.address} failed: ${e.message}")
            }
        }
    }

    /** Whatever has arrived so far, without waiting for more. */
    @Command
    fun read(invoke: Invoke) {
        val args = invoke.parseArgs(ReadArgs::class.java)
        val ret = JSObject()
        val link = links[args.address]
        val data = JSArray()
        if (link != null) {
            for (b in link.take(args.max)) data.put(b.toInt() and 0xff)
        }
        ret.put("data", data)
        ret.put("open", link?.open ?: false)
        invoke.resolve(ret)
    }

    @Command
    fun close(invoke: Invoke) {
        val args = invoke.parseArgs(AddressArgs::class.java)
        links.remove(args.address)?.close()
        keepAwakeWhileLinked()
        invoke.resolve(JSObject())
    }

    /**
     * Keep the screen on for as long as an adapter is connected.
     *
     * A phone left alone locks its screen, and Android then stops the app: a
     * scan or a recording simply ends, with nothing on screen to say so when
     * the phone is next picked up. A drive is exactly when nobody is touching
     * it. The flag belongs to the window, so it lapses by itself when the app
     * is closed or sent to the background.
     */
    private fun keepAwakeWhileLinked() {
        activity.runOnUiThread {
            // Looked up here, not by the caller: two threads report changes,
            // and the last one to run has to be the one that is right.
            if (links.values.any { it.open }) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
        }
    }

    /**
     * One open socket and the bytes its reader thread has collected.
     * [onClosed] runs when the reader stops, which is how an adapter that
     * went away by itself is noticed.
     */
    private class Link(val socket: BluetoothSocket, private val onClosed: () -> Unit) {
        private val buffer = ByteArrayOutputStream()

        @Volatile
        var open = true

        private val reader = Thread {
            val chunk = ByteArray(1024)
            try {
                val input = socket.inputStream
                while (true) {
                    val n = input.read(chunk)
                    if (n < 0) break
                    synchronized(buffer) { buffer.write(chunk, 0, n) }
                }
            } catch (_: IOException) {
                // Closed by us or by the adapter; `open` says which half knows.
            }
            open = false
            onClosed()
        }

        fun start() {
            reader.isDaemon = true
            reader.name = "obd-bluetooth-reader"
            reader.start()
        }

        fun take(max: Int): ByteArray = synchronized(buffer) {
            val all = buffer.toByteArray()
            val n = minOf(max, all.size)
            buffer.reset()
            if (n < all.size) buffer.write(all, n, all.size - n)
            all.copyOf(n)
        }

        fun close() {
            open = false
            try { socket.close() } catch (_: IOException) {}
        }
    }

    private companion object {
        const val PERMISSION_REQUEST = 4201
    }
}
