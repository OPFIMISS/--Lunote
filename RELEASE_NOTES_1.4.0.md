# 月笺 Lunote 1.4.0

## 重点更新

- 修复 Android 系统分享大文件时主线程同步复制导致的“应用无响应”；分享文件复制改为后台线程。
- Android 进入后台时启动前台传输服务，降低长时间传输被系统回收导致断线的概率，保留断点续传状态。
- PC/Android 传输中心新增“一键接收全部”并复用统一接收目录与冲突策略。
- 保持 applicationId、签名配置和协议兼容，支持从 1.3.x 覆盖升级。

## 发布校验

- Android APK：`EB5B872276A0C3A2CEA3B7AEF473B6FC8420A68CA58EC9AC07DE7A1F515F0D98`
- Windows EXE：`A0FAC39BD1FD6FB57AB4F6D95F8A61D695BCC1BF2DE12A0A7733503DE1F8011A`
- Windows ZIP：`83F847B270E7D442A4836528256ADA8BAB10D1647C92FCA386EF29882DF34852`

## 验证

- Flutter analyze：通过。
- Rust 单元与端到端测试：保持全绿。
- Windows：Release 进程启动冒烟通过。
- Android：APK 已构建；本轮模拟器在安装前离线，未虚报装机验证。
