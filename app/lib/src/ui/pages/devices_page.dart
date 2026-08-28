import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../state/app_state.dart';
import '../lunote_theme.dart';
import '../widgets/spring_button.dart';
import '../widgets/trust_dialog.dart';

/// 设备页：显示真实发现层上报的设备（在线/离线、信任状态、身份警告）。
class DevicesPage extends StatelessWidget {
  const DevicesPage({super.key, required this.onOpenConversation});

  final ValueChanged<String> onOpenConversation;

  Future<void> _refresh(BuildContext context) async {
    final state = context.read<AppState>();
    await state.refreshPeers();
    await state.refreshTrusted();
    if (!context.mounted) return;
    final online = state.peers.values.where((p) => p.online).length;
    final total = state.peers.length;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          total == 0 ? '已刷新，仍未发现设备' : '已刷新：在线 $online / 共 $total 台设备',
        ),
      ),
    );
  }

  Future<void> _manualConnect(BuildContext context) async {
    final hostController = TextEditingController();
    final portController = TextEditingController(text: '45455');
    var connecting = false;
    String? error;
    final deviceId = await showDialog<String>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) => AlertDialog(
          title: const Row(
            children: [
              Icon(Icons.cable_rounded),
              SizedBox(width: 10),
              Text('通过地址连接'),
            ],
          ),
          content: SizedBox(
            width: 380,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: hostController,
                  autofocus: true,
                  enabled: !connecting,
                  decoration: const InputDecoration(
                    labelText: 'IP 或主机名',
                    hintText: '例如 10.0.0.8',
                    prefixIcon: Icon(Icons.language_rounded),
                  ),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: portController,
                  enabled: !connecting,
                  keyboardType: TextInputType.number,
                  decoration: const InputDecoration(
                    labelText: '端口',
                    prefixIcon: Icon(Icons.numbers_rounded),
                  ),
                ),
                if (error != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 12),
                    child: Align(
                      alignment: Alignment.centerLeft,
                      child: Text(
                        error!,
                        style: TextStyle(
                          color: Theme.of(context).colorScheme.error,
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: connecting ? null : () => Navigator.pop(dialogContext),
              child: const Text('取消'),
            ),
            FilledButton.icon(
              onPressed: connecting
                  ? null
                  : () async {
                      final host = hostController.text.trim();
                      final port = int.tryParse(portController.text.trim());
                      if (host.isEmpty ||
                          port == null ||
                          port < 1 ||
                          port > 65535) {
                        setDialogState(() => error = '请输入有效的地址和端口');
                        return;
                      }
                      setDialogState(() {
                        connecting = true;
                        error = null;
                      });
                      final result = await context
                          .read<AppState>()
                          .connectAddress(host, port);
                      if (!dialogContext.mounted) return;
                      if (result.error != null) {
                        setDialogState(() {
                          connecting = false;
                          error = result.error;
                        });
                        return;
                      }
                      Navigator.pop(dialogContext, result.deviceId);
                    },
              icon: connecting
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.link_rounded),
              label: Text(connecting ? '连接中' : '连接'),
            ),
          ],
        ),
      ),
    );
    hostController.dispose();
    portController.dispose();
    if (deviceId != null && context.mounted) {
      onOpenConversation(deviceId);
    }
  }

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    final state = context.watch<AppState>();
    final devices = state.peers.values.toList()
      ..sort((a, b) {
        final af = state.favoriteDevices.contains(a.deviceId);
        final bf = state.favoriteDevices.contains(b.deviceId);
        if (af != bf) return af ? -1 : 1;
        if (a.online != b.online) return a.online ? -1 : 1;
        return a.name.compareTo(b.name);
      });
    final onlineCount = state.peers.values.where((p) => p.online).length;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 0),
          child: Row(
            children: [
              Text(
                '设备',
                style: TextStyle(
                  fontSize: 22,
                  fontWeight: FontWeight.w700,
                  color: cc.moon,
                ),
              ),
              const SizedBox(width: 10),
              Text(
                '在线 $onlineCount',
                style: TextStyle(fontSize: 12, color: cc.moonDim),
              ),
              const Spacer(),
              IconButton(
                onPressed: () => _manualConnect(context),
                tooltip: '通过 IP 或主机名连接',
                icon: Icon(Icons.add_link_rounded, color: cc.gold),
              ),
              IconButton(
                onPressed: () => _refresh(context),
                tooltip: '立即刷新设备列表',
                icon: Icon(Icons.refresh_rounded, color: cc.gold),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 4, 24, 12),
          child: Text(
            '局域网设备会自动出现，也可通过虚拟局域网地址直接连接',
            style: TextStyle(fontSize: 12.5, color: cc.moonDim),
          ),
        ),
        Expanded(
          child: RefreshIndicator(
            onRefresh: () => _refresh(context),
            color: cc.gold,
            child: devices.isEmpty
                ? LayoutBuilder(
                    builder: (context, constraints) => ListView(
                      physics: const AlwaysScrollableScrollPhysics(),
                      children: [
                        SizedBox(
                          height: constraints.maxHeight,
                          child: Center(
                            child: Column(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Icon(
                                  Icons.wifi_find,
                                  size: 44,
                                  color: cc.nightSoft,
                                ),
                                SizedBox(height: 12),
                                Text(
                                  '等待发现设备…',
                                  style: TextStyle(color: cc.moonDim),
                                ),
                                SizedBox(height: 4),
                                Text(
                                  '可下拉或点右上角刷新',
                                  style: TextStyle(
                                    fontSize: 11,
                                    color: cc.moonDim,
                                  ),
                                ),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  )
                : ListView.builder(
                    physics: const AlwaysScrollableScrollPhysics(),
                    padding: const EdgeInsets.fromLTRB(18, 6, 18, 18),
                    itemCount: devices.length,
                    itemBuilder: (context, i) {
                      final d = devices[i];
                      final trusted = state.isTrusted(d.deviceId);
                      final favorite = state.favoriteDevices.contains(d.deviceId);
                      final warning = state.identityWarnings[d.deviceId];
                      return GestureDetector(
                        behavior: HitTestBehavior.opaque,
                        onDoubleTap: () => onOpenConversation(d.deviceId),
                        child: Container(
                          margin: const EdgeInsets.symmetric(vertical: 5),
                          padding: const EdgeInsets.all(14),
                          decoration: BoxDecoration(
                            color: cc.nightRaised,
                            borderRadius: BorderRadius.circular(16),
                          ),
                          child: Row(
                            children: [
                              _avatar(cc, d.name, d.online),
                              const SizedBox(width: 12),
                              Expanded(
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Row(
                                      children: [
                                        Flexible(
                                          child: Text(
                                            d.name,
                                            maxLines: 1,
                                            overflow: TextOverflow.ellipsis,
                                            style: TextStyle(
                                              fontSize: 14.5,
                                              fontWeight: FontWeight.w600,
                                              color: cc.moon,
                                            ),
                                          ),
                                        ),
                                        const SizedBox(width: 8),
                                        Container(
                                          width: 7,
                                          height: 7,
                                          decoration: BoxDecoration(
                                            color: d.online
                                                ? cc.online
                                                : cc.offline,
                                            shape: BoxShape.circle,
                                          ),
                                        ),
                                      ],
                                    ),
                                    const SizedBox(height: 3),
                                    Text(
                                      '${d.ip}${d.online ? '' : ' · 离线'}',
                                      style: TextStyle(
                                        fontSize: 12,
                                        color: cc.moonDim,
                                      ),
                                    ),
                                    if (warning != null)
                                      Padding(
                                        padding: const EdgeInsets.only(top: 3),
                                        child: Text(
                                          '⚠ 身份指纹变化，请重新确认信任',
                                          style: TextStyle(
                                            fontSize: 11.5,
                                            color: cc.warn,
                                          ),
                                        ),
                                      ),
                                  ],
                                ),
                              ),
                              IconButton(
                                onPressed: () => onOpenConversation(d.deviceId),
                                tooltip: '打开与「${d.name}」的对话',
                                icon: Icon(
                                  Icons.chat_bubble_rounded,
                                  size: 20,
                                  color: cc.gold,
                                ),
                              ),
                              const SizedBox(width: 4),
                              _TrustAction(
                                trusted: trusted,
                                onTrust: () async {
                                  final err = await state.trustDevice(
                                    d.deviceId,
                                    trusted: true,
                                    name: d.name,
                                  );
                                  if (context.mounted) {
                                    ScaffoldMessenger.of(context).showSnackBar(
                                      SnackBar(
                                        content: Text(
                                          err == null
                                              ? '已信任「${d.name}」'
                                              : '信任失败：$err',
                                        ),
                                      ),
                                    );
                                  }
                                },
                                onShow: () async {
                                  final ok = await showTrustDialog(
                                    context,
                                    state,
                                    d.deviceId,
                                  );
                                  if (ok == true && context.mounted) {
                                    final err = await state.trustDevice(
                                      d.deviceId,
                                      trusted: true,
                                      name: d.name,
                                    );
                                    if (context.mounted) {
                                      ScaffoldMessenger.of(context)
                                          .showSnackBar(
                                            SnackBar(
                                              content: Text(
                                                err == null
                                                    ? '已信任「${d.name}」'
                                                    : '信任失败：$err',
                                              ),
                                            ),
                                          );
                                    }
                                  }
                                },
                              ),
                              if (true)
                                PopupMenuButton<String>(
                                  tooltip: '设备操作',
                                  icon: Icon(Icons.more_vert_rounded, color: cc.moonDim),
                                  onSelected: (value) async {
                                    if (value == 'favorite') {
                                      final err = await state.setDeviceMeta(d.deviceId, favorite: !favorite);
                                      if (context.mounted && err != null) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err)));
                                      return;
                                    }
                                    if (value == 'alias') {
                                      final ctrl = TextEditingController(text: state.deviceAliases[d.deviceId] ?? '');
                                      final alias = await showDialog<String>(context: context, builder: (ctx) => AlertDialog(title: const Text('设备备注'), content: TextField(controller: ctrl, autofocus: true, decoration: const InputDecoration(hintText: '例如：客厅电脑')), actions: [TextButton(onPressed: () => Navigator.pop(ctx), child: const Text('取消')), FilledButton(onPressed: () => Navigator.pop(ctx, ctrl.text.trim()), child: const Text('保存'))]));
                                      if (alias != null) { final err = await state.setDeviceMeta(d.deviceId, alias: alias); if (context.mounted && err != null) ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(err))); }
                                      return;
                                    }
                                    if (value != 'remove' || !context.mounted) return;
                                    final ok = await showDialog<bool>(
                                      context: context,
                                      builder: (ctx) => AlertDialog(
                                        title: const Text('移除设备记录？'),
                                        content: Text(
                                          '将移除「${d.name}」的本地身份/信任记录，不会影响对方设备，也不会删除对话。',
                                        ),
                                        actions: [
                                          TextButton(
                                            onPressed: () => Navigator.pop(ctx, false),
                                            child: const Text('取消'),
                                          ),
                                          FilledButton(
                                            onPressed: () => Navigator.pop(ctx, true),
                                            child: const Text('移除'),
                                          ),
                                        ],
                                      ),
                                    );
                                    if (ok != true || !context.mounted) return;
                                    final err = await state.removeDevice(d.deviceId);
                                    if (context.mounted) {
                                      ScaffoldMessenger.of(context).showSnackBar(
                                        SnackBar(content: Text(err == null ? '已移除设备记录' : '移除失败：$err')),
                                      );
                                    }
                                  },
                                  itemBuilder: (_) => [
                                    PopupMenuItem(value: 'favorite', child: Text(favorite ? '取消收藏' : '收藏设备')),
                                    PopupMenuItem(value: 'alias', child: Text('添加备注')),
                                    PopupMenuItem(value: 'remove', child: Text('移除设备记录')),
                                  ],
                                ),
                            ],
                          ),
                        ),
                      );
                    },
                  ),
          ),
        ),
      ],
    );
  }

  Widget _avatar(LunoteColors cc, String name, bool online) {
    final char = name.isEmpty
        ? '?'
        : String.fromCharCode(name.runes.first).toUpperCase();
    return Stack(
      children: [
        Container(
          width: 42,
          height: 42,
          decoration: BoxDecoration(
            color: cc.nightSoft,
            shape: BoxShape.circle,
          ),
          alignment: Alignment.center,
          child: Text(
            char,
            style: TextStyle(
              fontSize: 17,
              color: cc.gold,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        Positioned(
          right: 0,
          bottom: 0,
          child: Container(
            width: 11,
            height: 11,
            decoration: BoxDecoration(
              color: online ? cc.online : cc.offline,
              shape: BoxShape.circle,
              border: Border.all(color: cc.nightRaised, width: 2),
            ),
          ),
        ),
      ],
    );
  }
}

class _TrustAction extends StatelessWidget {
  const _TrustAction({
    required this.trusted,
    required this.onTrust,
    required this.onShow,
  });

  final bool trusted;
  final VoidCallback onTrust;
  final VoidCallback onShow;

  @override
  Widget build(BuildContext context) {
    final cc = LunoteColors.of(context);
    if (trusted) {
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        decoration: BoxDecoration(
          color: const Color(0x227BD389),
          borderRadius: BorderRadius.circular(20),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.verified_rounded, size: 14, color: cc.online),
            SizedBox(width: 4),
            Text('可信', style: TextStyle(fontSize: 12, color: cc.online)),
          ],
        ),
      );
    }
    return SpringButton(
      weight: SpringWeight.primary,
      onTap: onShow,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 7),
        decoration: BoxDecoration(
          color: cc.gold,
          borderRadius: BorderRadius.circular(20),
        ),
        child: Text(
          '信任',
          style: TextStyle(
            fontSize: 12.5,
            fontWeight: FontWeight.w700,
            color: cc.night,
          ),
        ),
      ),
    );
  }
}
