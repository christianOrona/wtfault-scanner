package com.wtfault.scanner

import android.app.Activity
import android.content.ClipData
import android.content.Intent
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

/**
 * Hands one file from the app's cache to the share sheet, for the Rust core.
 *
 * The app's storage is private, so this is the only way a recorded session
 * leaves the phone: the person picks where it goes, and that app is granted a
 * read of this one file and nothing else. See `src/android_share.rs` for the
 * other half.
 */
@TauriPlugin
class SharePlugin(private val activity: Activity) : Plugin(activity) {

    @InvokeArg
    class ShareArgs {
        lateinit var path: String
        var mime: String = "application/octet-stream"
    }

    /**
     * Open the share sheet for a file. Resolves once the sheet is up: Android
     * does not say what the person picked, or whether they picked anything.
     */
    @Command
    fun share(invoke: Invoke) {
        val args = invoke.parseArgs(ShareArgs::class.java)
        try {
            val file = File(args.path)
            if (!file.isFile) {
                invoke.reject("there is no file at ${args.path}")
                return
            }
            // Throws for a file outside the folders listed in file_paths.xml,
            // which is what keeps this from handing over the database itself.
            val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", file)
            val send = Intent(Intent.ACTION_SEND).apply {
                type = args.mime
                putExtra(Intent.EXTRA_STREAM, uri)
                putExtra(Intent.EXTRA_SUBJECT, file.name)
                // The chooser's own preview reads the file too, and takes its
                // permission from the clip data rather than the extra.
                clipData = ClipData.newRawUri(file.name, uri)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            activity.startActivity(Intent.createChooser(send, file.name))
            invoke.resolve(JSObject())
        } catch (e: Exception) {
            invoke.reject("could not open the share sheet: ${e.message ?: e.javaClass.simpleName}")
        }
    }
}
