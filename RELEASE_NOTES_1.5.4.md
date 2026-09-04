# Lunote 1.5.4

## 修复

- Android“打开所在文件夹”现在可直接进入 SAF 选定目录。
- Android 目录 URI 显示为可读的目录名称，不再展示完整 content URI。
- Android 相册按钮改用系统照片选择器，支持多选图片。
- 图片预览按原始比例显示，修复长图、竖图被裁剪的问题。
- SAF 大文件导出移至后台线程，降低界面无响应风险。
- 接收失败时保留可续传分片，并提供明确错误提示。

## 构建

- Android versionName：`1.5.4`
- Android versionCode：`11`
- applicationId 与 Release 签名配置保持不变，可覆盖安装。
