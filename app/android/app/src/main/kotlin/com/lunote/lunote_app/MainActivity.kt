package com.lunote.lunote_app

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import androidx.core.content.FileProvider
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/**
 * 月笺 Lunote 主活动。
 *
 * 组播锁（MulticastLock）：Android 的 Wi-Fi 节能默认过滤组播包，
 * 不加锁则收不到局域网发现信标（UDP 组播 239.255.77.77）。
 * 权限 CHANGE_WIFI_MULTICAST_STATE 已在 AndroidManifest.xml 声明。
 */
class MainActivity : FlutterActivity() {
    private val platformChannel = "com.lunote.lunote_app/platform"
    private var multicastLock: WifiManager.MulticastLock? = null
    private var pendingIntent: Intent? = null
    private var pendingFolderResult: MethodChannel.Result? = null
    private val folderRequestCode = 4207

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            platformChannel,
        ).setMethodCallHandler { call, result ->
            val path = call.argument<String>("path").orEmpty()
            when (call.method) {
                "getDeviceModel" -> result.success(deviceModel())
                "openDirectory" -> result.success(openDirectory(path))
                "openFile" -> result.success(openFile(path))
                "getPendingShare" -> result.success(readShareIntent(pendingIntent ?: intent).also { pendingIntent = null })
                "pickFolderForTransfer" -> pickFolderForTransfer(result)
                else -> result.notImplemented()
            }
        }
        pendingIntent = intent
    }

    private fun deviceModel(): String {
        val manufacturer = Build.MANUFACTURER.trim()
        val model = Build.MODEL.trim()
        val raw = if (manufacturer.isNotEmpty() &&
            !model.startsWith(manufacturer, ignoreCase = true)
        ) "$manufacturer $model" else model
        return raw.replace(Regex("\\s+"), " ").trim().ifEmpty { "Android 设备" }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        pendingIntent = intent
    }

    private fun readShareIntent(source: Intent): Map<String, Any?>? {
        val action = source.action ?: return null
        if (action != Intent.ACTION_SEND && action != Intent.ACTION_SEND_MULTIPLE) return null
        val text = source.getStringExtra(Intent.EXTRA_TEXT)
        val uri = source.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
        val uris = if (Build.VERSION.SDK_INT >= 33) {
            source.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            source.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
        }
        val selectedUris = when {
            action == Intent.ACTION_SEND_MULTIPLE -> uris.orEmpty()
            uri != null -> listOf(uri)
            else -> emptyList()
        }
        val paths = selectedUris.mapIndexedNotNull { index, item ->
            copySharedUri(item, index)
        }
        if (text.isNullOrBlank() && paths.isEmpty()) return null
        return mapOf(
            "text" to text,
            "paths" to paths,
            // 兼容旧版 Flutter 读取字段，同时让单文件分享保持原有行为。
            "path" to paths.firstOrNull(),
            "name" to paths.firstOrNull()?.let { java.io.File(it).name },
        )
    }

    private fun copySharedUri(uri: Uri, index: Int = 0): String? {
        return try {
            val name = queryDisplayName(uri) ?: "shared-${System.currentTimeMillis()}"
            val safe = name.replace(Regex("[^A-Za-z0-9._\\-()\\u4e00-\\u9fff ]"), "_")
            val out = java.io.File(cacheDir, "shared/${System.currentTimeMillis()}-$index-$safe")
            out.parentFile?.mkdirs()
            contentResolver.openInputStream(uri)?.use { input -> out.outputStream().use { input.copyTo(it) } }
                ?: return null
            out.absolutePath
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法读取分享文件: ${e.message}")
            null
        }
    }

    private fun queryDisplayName(uri: Uri): String? {
        contentResolver.query(uri, arrayOf("_display_name"), null, null, null)?.use { c ->
            if (c.moveToFirst()) return c.getString(0)
        }
        return uri.lastPathSegment?.substringAfterLast('/')
    }

    private fun pickFolderForTransfer(result: MethodChannel.Result) {
        if (pendingFolderResult != null) {
            result.error("BUSY", "已有文件夹选择正在进行", null)
            return
        }
        pendingFolderResult = result
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
            putExtra("android.content.extra.SHOW_ADVANCED", true)
        }
        startActivityForResult(intent, folderRequestCode)
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != folderRequestCode) return
        val result = pendingFolderResult ?: return
        pendingFolderResult = null
        if (resultCode != RESULT_OK || data?.data == null) {
            result.success(null)
            return
        }
        val uri = data.data!!
        try {
            contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
        } catch (_: Exception) { }
        Thread {
            val copied = copyTree(uri)
            runOnUiThread { result.success(copied) }
        }.start()
    }

    private fun copyTree(tree: Uri): String? {
        return try {
            val root = java.io.File(cacheDir, "shared-folder/${System.currentTimeMillis()}")
            root.mkdirs()
            copyTreeNode(tree, root)
            root.absolutePath
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法读取分享文件夹: ${e.message}")
            null
        }
    }

    private fun copyTreeNode(tree: Uri, target: java.io.File) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(
            tree, DocumentsContract.getTreeDocumentId(tree)
        )
        contentResolver.query(children, arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        ), null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val id = c.getString(0)
                val name = c.getString(1).replace(Regex("[\\\\/:*?\"<>|]"), "_")
                val mime = c.getString(2)
                val child = DocumentsContract.buildDocumentUriUsingTree(tree, id)
                val out = java.io.File(target, name)
                if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
                    out.mkdirs(); copyTreeNode(child, out)
                } else {
                    contentResolver.openInputStream(child)?.use { input -> out.outputStream().use { input.copyTo(it) } }
                }
            }
        }
    }

    private fun openFile(path: String): Boolean {
        return try {
            val file = java.io.File(path)
            if (!file.isFile) return false
            val uri = FileProvider.getUriForFile(
                this,
                "$packageName.fileprovider",
                file,
            )
            val mime = java.net.URLConnection.guessContentTypeFromName(file.name) ?: "*/*"
            val intent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, mime)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            startActivity(Intent.createChooser(intent, "打开文件"))
            true
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法打开文件: ${e.message}")
            false
        }
    }

    private fun openDirectory(path: String): Boolean {
        return try {
            val directory = java.io.File(path)
            if (!directory.exists() && !directory.mkdirs()) return false
            val uri = FileProvider.getUriForFile(
                this,
                "$packageName.fileprovider",
                directory,
            )
            val viewIntent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, "resource/folder")
                addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }
            if (viewIntent.resolveActivity(packageManager) != null) {
                startActivity(Intent.createChooser(viewIntent, "打开文件夹"))
                return true
            }

            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
                addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                        Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION,
                )
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    initialDocumentUri(path)?.let {
                        putExtra(DocumentsContract.EXTRA_INITIAL_URI, it)
                    }
                }
            }
            startActivity(intent)
            true
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法打开接收目录: ${e.message}")
            false
        }
    }

    private fun initialDocumentUri(path: String): Uri? {
        val normalized = path.replace('\\', '/')
        val marker = "/storage/emulated/0/"
        if (!normalized.startsWith(marker)) return null
        val relative = normalized.removePrefix(marker).trim('/')
        val documentId = if (relative.isEmpty()) "primary:" else "primary:$relative"
        return DocumentsContract.buildDocumentUri(
            "com.android.externalstorage.documents",
            documentId,
        )
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        acquireMulticastLock()
    }

    override fun onResume() {
        super.onResume()
        // 部分 Android ROM 在 Activity 恢复后会释放 Wi-Fi 组播接收能力。
        acquireMulticastLock()
    }

    private fun acquireMulticastLock() {
        try {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            val lock = multicastLock ?: wifi.createMulticastLock("lunote-discovery").apply {
                setReferenceCounted(false)
            }
            multicastLock = lock
            if (!lock.isHeld) lock.acquire()
        } catch (e: Exception) {
            // 获取失败不阻塞应用：发现可退化为仅广播模式
            android.util.Log.w("Lunote", "无法获取组播锁: ${e.message}")
        }
    }

    override fun onDestroy() {
        try {
            multicastLock?.release()
        } catch (_: Exception) {
        }
        super.onDestroy()
    }
}
