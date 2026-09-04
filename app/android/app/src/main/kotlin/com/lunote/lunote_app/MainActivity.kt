package com.lunote.lunote_app

import android.content.Context
import android.content.ClipData
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.app.PendingIntent
import androidx.core.content.FileProvider
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
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
    private var pendingTransferId: String? = null
    private var pendingTransferAction: String? = null
    private var pendingFolderResult: MethodChannel.Result? = null
    private var pendingReceiveFolderResult: MethodChannel.Result? = null
    private var pendingGalleryResult: MethodChannel.Result? = null
    private var pendingApkPath: String? = null
    private val folderRequestCode = 4207
    private val receiveFolderRequestCode = 4208
    private val galleryRequestCode = 4209
    private val transferChannelId = "lunote_transfers"

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            platformChannel,
        ).setMethodCallHandler { call, result ->
            val path = call.argument<String>("path").orEmpty()
            when (call.method) {
                "getDeviceModel" -> result.success(deviceModel())
                "notifyTransfer" -> {
                    notifyTransfer(
                        call.argument<String>("title") ?: "月笺传输",
                        call.argument<String>("body") ?: "",
                        call.argument<String>("transfer_id"),
                    )
                    result.success(true)
                }
                "requestNotificationPermission" -> {
                    if (Build.VERSION.SDK_INT >= 33 &&
                        checkSelfPermission("android.permission.POST_NOTIFICATIONS") !=
                            android.content.pm.PackageManager.PERMISSION_GRANTED
                    ) {
                        requestPermissions(arrayOf("android.permission.POST_NOTIFICATIONS"), 9301)
                    }
                    result.success(true)
                }
                "openDirectory" ->
                    result.success(openDirectory(path, call.argument<String>("treeUri")))
                "openFile" -> result.success(openFile(path))
                "getPendingShare" -> {
                    // Copying a shared 4 GB file can take minutes. Never perform it on
                    // Flutter's platform thread or Android will show an ANR dialog.
                    val source = pendingIntent ?: intent
                    pendingIntent = null
                    readShareIntentAsync(source, result)
                }
                "getPendingTransferId" -> result.success(pendingTransferId.also { pendingTransferId = null })
                "getPendingTransferAction" -> result.success(pendingTransferAction.also { pendingTransferAction = null })
                "pickFolderForTransfer" -> pickFolderForTransfer(result)
                "pickReceiveFolder" -> pickReceiveFolder(result)
                "pickGallery" -> pickGallery(result)
                "exportToTree" -> exportToTreeAsync(
                    path,
                    call.argument<String>("treeUri"),
                    result,
                )
                else -> result.notImplemented()
            }
        }
        pendingIntent = intent
        pendingTransferId = intent.getStringExtra("transfer_id")
        pendingTransferAction = intent.getStringExtra("transfer_action")
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
        pendingTransferId = intent.getStringExtra("transfer_id")
        pendingTransferAction = intent.getStringExtra("transfer_action")
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

    private fun readShareIntentAsync(source: Intent, result: MethodChannel.Result) {
        Thread {
            val value = readShareIntent(source)
            runOnUiThread { result.success(value) }
        }.apply {
            name = "lunote-share-copy"
            isDaemon = true
            start()
        }
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

    private fun pickReceiveFolder(result: MethodChannel.Result) {
        if (pendingReceiveFolderResult != null) { result.error("BUSY", "已有目录选择正在进行", null); return }
        pendingReceiveFolderResult = result
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
            putExtra("android.content.extra.SHOW_ADVANCED", true)
        }
        startActivityForResult(intent, receiveFolderRequestCode)
    }

    private fun pickGallery(result: MethodChannel.Result) {
        if (pendingGalleryResult != null) {
            result.error("BUSY", "已有相册选择正在进行", null)
            return
        }
        pendingGalleryResult = result
        val intent = Intent(
            if (Build.VERSION.SDK_INT >= 33) {
                // Android 13+ 系统照片选择器，避免回落到 DocumentsUI 文件管理器。
                "android.provider.action.PICK_IMAGES"
            } else {
                Intent.ACTION_PICK
            },
        ).apply {
            if (Build.VERSION.SDK_INT < 33) {
                setDataAndType(
                    android.provider.MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                    "image/*",
                )
            } else {
                type = "image/*"
                putExtra("android.provider.extra.PICK_IMAGES_MAX", 100)
            }
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        try {
            startActivityForResult(intent, galleryRequestCode)
        } catch (e: Exception) {
            pendingGalleryResult = null
            result.error("NO_GALLERY", "系统没有可用的相册应用", e.message)
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == receiveFolderRequestCode) {
            val result = pendingReceiveFolderResult ?: return
            pendingReceiveFolderResult = null
            if (resultCode != RESULT_OK || data?.data == null) { result.success(null); return }
            val uri = data.data!!
            try { contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION) } catch (_: Exception) { }
            result.success(uri.toString())
            return
        }
        if (requestCode == galleryRequestCode) {
            val result = pendingGalleryResult ?: return
            pendingGalleryResult = null
            if (resultCode != RESULT_OK || data == null) {
                result.success(emptyList<String>())
                return
            }
            val selected = mutableListOf<Uri>()
            data.data?.let { selected.add(it) }
            data.clipData?.let { clip ->
                for (i in 0 until clip.itemCount) selected.add(clip.getItemAt(i).uri)
            }
            Thread {
                val paths = selected.distinct().mapIndexedNotNull { index, uri ->
                    copySharedUri(uri, index)
                }
                runOnUiThread { result.success(paths) }
            }.apply {
                name = "lunote-gallery-copy"
                isDaemon = true
                start()
            }
            return
        }
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

    private fun exportToTree(path: String, treeUriString: String?): Boolean {
        if (treeUriString.isNullOrBlank()) return false
        return try {
            val tree = Uri.parse(treeUriString)
            val name = java.io.File(path).name
            val doc = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
            val mime = java.net.URLConnection.guessContentTypeFromName(name) ?: "application/octet-stream"
            val target = DocumentsContract.createDocument(contentResolver, doc, mime, name) ?: return false
            contentResolver.openOutputStream(target)?.use { out -> java.io.File(path).inputStream().use { it.copyTo(out) } } ?: return false
            true
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "导出到 SAF 目录失败: ${e.message}")
            false
        }
    }

    private fun exportToTreeAsync(
        path: String,
        treeUriString: String?,
        result: MethodChannel.Result,
    ) {
        // SAF 流复制可能持续数分钟，尤其是数 GB 文件。必须离开平台主线程，
        // 否则 Flutter 界面与系统 ANR 检测都会被阻塞。
        Thread {
            val exported = exportToTree(path, treeUriString)
            runOnUiThread { result.success(exported) }
        }.apply {
            name = "lunote-saf-export"
            isDaemon = true
            start()
        }
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
            val isApk = file.name.lowercase().endsWith(".apk")
            if (isApk) {
                // 安装器需要 REQUEST_INSTALL_PACKAGES 与“安装未知应用”授权；
                // 未授权时直接拉起会被系统静默拒绝（表现为点了安装程序却没反应）。
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
                    !packageManager.canRequestPackageInstalls()
                ) {
                    pendingApkPath = file.absolutePath
                    return openUnknownSourcesSettings()
                }
                val install = Intent(Intent.ACTION_INSTALL_PACKAGE).apply {
                    setDataAndType(uri, "application/vnd.android.package-archive")
                    clipData = ClipData.newRawUri("Lunote APK", uri)
                    addFlags(
                        Intent.FLAG_GRANT_READ_URI_PERMISSION or
                            Intent.FLAG_ACTIVITY_NEW_TASK,
                    )
                }
                val handlers = packageManager.queryIntentActivities(install, 0)
                for (handler in handlers) {
                    grantUriPermission(
                        handler.activityInfo.packageName,
                        uri,
                        Intent.FLAG_GRANT_READ_URI_PERMISSION,
                    )
                }
                return try {
                    // Android 11+ 的包可见性可能令 resolveActivity/query 返回空，
                    // 但系统安装器仍能处理隐式 Intent，因此直接尝试启动。
                    startActivity(install)
                    true
                } catch (_: Exception) {
                    install.action = Intent.ACTION_VIEW
                    startActivity(install)
                    true
                }
            }
            val mime = java.net.URLConnection.guessContentTypeFromName(file.name) ?: "*/*"
            val intent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, mime)
                addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        Intent.FLAG_ACTIVITY_NEW_TASK,
                )
            }
            startActivity(Intent.createChooser(intent, "打开文件"))
            true
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法打开文件: ${e.message}")
            false
        }
    }

    private fun openUnknownSourcesSettings(): Boolean {
        try {
            val intent = Intent(
                android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                Uri.parse("package:$packageName"),
            ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (intent.resolveActivity(packageManager) != null) {
                startActivity(intent)
                return true
            }
        } catch (_: Exception) {
        }
        try {
            startActivity(
                Intent(android.provider.Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            return true
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法打开安装权限设置: ${e.message}")
            return false
        }
    }

    private fun openDirectory(path: String, treeUriString: String?): Boolean {
        // 1) 接收文件在 Android 上先写私有暂存、再经 SAF 导出到用户选择的目录；
        //    查看“所在文件夹”时优先打开该 SAF 导出目录的真实内容视图。
        if (!treeUriString.isNullOrBlank()) {
            try {
                val tree = Uri.parse(treeUriString)
                val authority = tree.authority
                    ?: "com.android.externalstorage.documents"
                val documentId = DocumentsContract.getTreeDocumentId(tree)
                // 部分 ROM 对 tree URI 的 VIEW 会误判为目录选择器；直接使用
                // document URI 可稳定进入目标目录。
                val dirDoc = DocumentsContract.buildDocumentUri(authority, documentId)
                val view = Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(dirDoc, DocumentsContract.Document.MIME_TYPE_DIR)
                    addFlags(
                        Intent.FLAG_GRANT_READ_URI_PERMISSION or
                            Intent.FLAG_ACTIVITY_NEW_TASK,
                    )
                }
                if (view.resolveActivity(packageManager) != null) {
                    startActivity(view)
                    return true
                }
            } catch (e: Exception) {
                android.util.Log.w("Lunote", "无法查看 SAF 接收目录: ${e.message}")
            }
        }
        // 2) 公共外部存储的真实路径：映射为 externalstorage 文档 URI 后以浏览模式打开。
        try {
            externalDocUriForPath(path)?.let { doc ->
                val view = Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(doc, DocumentsContract.Document.MIME_TYPE_DIR)
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                if (view.resolveActivity(packageManager) != null) {
                    startActivity(view)
                    return true
                }
            }
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法查看外部目录: ${e.message}")
        }
        // 3) 部分国产文件管理器支持 resource/folder 协议。
        try {
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
                        Intent.FLAG_ACTIVITY_NEW_TASK,
                )
            }
            if (viewIntent.resolveActivity(packageManager) != null) {
                startActivity(viewIntent)
                return true
            }
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法打开文件夹: ${e.message}")
        }
        // 不再回退到 ACTION_OPEN_DOCUMENT_TREE——那是“选择目录”界面而不是查看目录。
        return false
    }

    private fun externalDocUriForPath(path: String): Uri? {
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
        createTransferNotificationChannel()
        acquireMulticastLock()
    }

    private fun createTransferNotificationChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = android.app.NotificationChannel(
            transferChannelId,
            "文件传输",
            android.app.NotificationManager.IMPORTANCE_DEFAULT,
        ).apply { description = "月笺文件传输状态" }
        getSystemService(android.app.NotificationManager::class.java)
            ?.createNotificationChannel(channel)
    }

    private fun notifyTransfer(title: String, body: String, transferId: String?) {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission("android.permission.POST_NOTIFICATIONS") !=
                android.content.pm.PackageManager.PERMISSION_GRANTED
        ) return
        val openIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            if (!transferId.isNullOrBlank()) putExtra("transfer_id", transferId)
        }
        val contentIntent = PendingIntent.getActivity(
            this, 9302, openIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = NotificationCompat.Builder(this, transferChannelId)
            .setSmallIcon(android.R.drawable.stat_sys_download_done)
            .setContentTitle(title)
            .setContentText(body)
            .setContentIntent(contentIntent)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
        if (!transferId.isNullOrBlank() && title.startsWith("收到文件")) {
            val baseCode = transferId.hashCode() and 0x7fffffff
            val acceptIntent = Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
                putExtra("transfer_id", transferId)
                putExtra("transfer_action", "accept")
            }
            val rejectIntent = Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
                putExtra("transfer_id", transferId)
                putExtra("transfer_action", "reject")
            }
            val acceptPending = PendingIntent.getActivity(
                this, baseCode + 1, acceptIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            val rejectPending = PendingIntent.getActivity(
                this, baseCode + 2, rejectIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            builder.addAction(NotificationCompat.Action(android.R.drawable.ic_input_add, "接收", acceptPending))
            builder.addAction(NotificationCompat.Action(android.R.drawable.ic_delete, "拒绝", rejectPending))
        }
        val notification = builder.build()
        NotificationManagerCompat.from(this).notify((System.currentTimeMillis() % 100000).toInt(), notification)
    }

    override fun onResume() {
        super.onResume()
        // 部分 Android ROM 在 Activity 恢复后会释放 Wi-Fi 组播接收能力。
        acquireMulticastLock()
        stopService(Intent(this, TransferForegroundService::class.java))
        val apk = pendingApkPath
        if (apk != null &&
            (Build.VERSION.SDK_INT < Build.VERSION_CODES.O ||
                packageManager.canRequestPackageInstalls())
        ) {
            pendingApkPath = null
            window.decorView.postDelayed({ openFile(apk) }, 250)
        }
    }

    override fun onPause() {
        // A foreground service keeps the Flutter/Rust process alive while Android
        // moves the Activity to the background during a long file transfer.
        try {
            val serviceIntent = Intent(this, TransferForegroundService::class.java)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(serviceIntent)
            } else {
                startService(serviceIntent)
            }
        } catch (e: Exception) {
            android.util.Log.w("Lunote", "无法启动后台传输服务: ${e.message}")
        }
        super.onPause()
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
