import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../../state/app_state.dart';
import '../../core/models.dart';
import '../lunote_theme.dart';
import '../widgets/lunote_mark.dart';
import '../widgets/spring_button.dart';
import '../widgets/trust_dialog.dart';
import 'chat_page.dart';
import 'devices_page.dart';
import 'settings_page.dart';
import 'transfers_page.dart';

enum _Nav { devices, conversations, transfers, settings }

/// 应用壳：宽屏（桌面）左侧舒展导航；窄屏（手机）底部导航 + 全屏对话。
class ShellPage extends StatefulWidget {
  const ShellPage({super.key});

  @override
  State<ShellPage> createState() => _ShellPageState();
}

class _ShellPageState extends State<ShellPage> with WidgetsBindingObserver {
  _Nav _nav = _Nav.devices;
  String? _activeDevice;
  final Set<String> _selectedConversations = {};
  bool _conversationSelectionMode = false;
  bool _handlingShare = false;

  bool get _selectingConversations => _conversationSelectionMode;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    // 新设备连接 → 弹出信任确认；已自动信任（同名同 IP）则只提示
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _handlePendingShare();
      _handlePendingTransfer();
      AppState.instance.core.events.listen((e) {
        if (e['event'] == 'peer_connected' &&
            e['is_new_device'] == true &&
            mounted) {
          final id = e['device_id'] as String?;
          if (id != null) {
            final state = AppState.instance;
            if (e['auto_trusted'] == true) {
              ScaffoldMessenger.of(context).showSnackBar(
                SnackBar(content: Text('已自动信任「${e['name'] ?? '设备'}」（同名同 IP）')),
              );
            } else if (!state.isTrusted(id)) {
              // 对话框确认后必须真正执行信任（此前返回值被忽略导致信任从不生效）
              showTrustDialog(context, state, id).then((ok) async {
                if (ok == true) {
                  final err = await state.trustDevice(id, trusted: true);
                  if (err != null && mounted) {
                    ScaffoldMessenger.of(context)
                        .showSnackBar(SnackBar(content: Text('信任失败：$err')));
                  }
                }
              });
            }
          }
        }
      });
    });
  }

  Future<void> _handlePendingTransfer() async {
    try {
      const channel = MethodChannel('com.lunote.lunote_app/platform');
      final id = await channel.invokeMethod<String>('getPendingTransferId');
      final action = await channel.invokeMethod<String>('getPendingTransferAction');
      if (!mounted || id == null || id.isEmpty) return;
      setState(() => _nav = _Nav.transfers);
      if (action == 'reject') {
        final error = await context.read<AppState>().rejectTransfer(id, '通过通知拒绝');
        if (error != null && mounted) {
          ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(error)));
        }
      }
    } on MissingPluginException {
      // Desktop does not provide Android notification actions.
    } catch (_) {}
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      _handlePendingShare();
      _handlePendingTransfer();
    }
  }

  Future<void> _handlePendingShare() async {
    if (!mounted || _handlingShare) return;
    _handlingShare = true;
    try {
      const channel = MethodChannel('com.lunote.lunote_app/platform');
      final raw = await channel.invokeMethod<Map<dynamic, dynamic>>('getPendingShare');
      if (!mounted || raw == null) return;
      final text = raw['text'] as String?;
      final sharedPaths = ((raw['paths'] as List?) ?? const [])
          .whereType<String>()
          .where((p) => p.isNotEmpty)
          .toList();
      final legacyPath = raw['path'] as String?;
      final paths = sharedPaths.isNotEmpty
          ? sharedPaths
          : (legacyPath == null || legacyPath.isEmpty ? const <String>[] : [legacyPath]);
      final state = context.read<AppState>();
      final peers = await _waitForOnlinePeers(state);
      if (!mounted) return;
      if (peers.isEmpty) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('已收到分享内容，但当前没有在线设备')),
        );
        return;
      }
      if (!mounted) return;
      final selected = await showDialog<PeerInfo>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          title: Text(paths.isEmpty ? '分享文字到设备' : '分享 ${paths.length} 个文件到设备'),
          content: SizedBox(
            width: 360,
            child: ListView.builder(
              shrinkWrap: true,
              itemCount: peers.length,
              itemBuilder: (_, i) {
                final peer = peers[i];
                return ListTile(
                  leading: const Icon(Icons.devices_rounded),
                  title: Text(peer.name),
                  subtitle: Text('${peer.ip}:${peer.tcpPort}'),
                  onTap: () => Navigator.of(dialogContext).pop(peer),
                );
              },
            ),
          ),
          actions: [TextButton(onPressed: () => Navigator.pop(dialogContext), child: const Text('取消'))],
        ),
      );
      if (!mounted || selected == null) return;
      String? error;
      if (paths.isNotEmpty) {
        for (final path in paths) {
          error = await state.sendFile(selected.deviceId, path);
          if (error != null) break;
        }
      } else if (text != null && text.trim().isNotEmpty) {
        final value = text.trim();
        error = value.startsWith('http://') || value.startsWith('https://')
            ? await state.sendLink(selected.deviceId, value)
            : await state.sendText(selected.deviceId, value);
      }
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text(error == null ? '已开始传输' : '发送失败：$error')),
      );
    } on MissingPluginException {
      // Windows/Linux 没有 Android 分享入口。
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('处理分享内容失败：$e')),
        );
      }
    } finally {
      _handlingShare = false;
    }
  }

  /// 分享 Intent 往往早于发现快照到达；短时重试避免第二次分享时列表暂为空。
  Future<List<PeerInfo>> _waitForOnlinePeers(AppState state) async {
    for (var attempt = 0; attempt < 12; attempt++) {
      try {
        await state.refreshPeers();
      } catch (_) {
        // 核心刚恢复时单次快照可能失败，下一轮继续尝试。
      }
      final peers = state.peers.values.where((p) => p.online).toList();
      if (peers.isNotEmpty) return peers;
      if (attempt < 11) {
        await Future<void>.delayed(const Duration(milliseconds: 300));
      }
    }
    return const [];
  }

  bool get _isWide => MediaQuery.sizeOf(context).width >= 700;

  @override
  Widget build(BuildContext context) {
    final state = context.watch<AppState>();
    final pendingTransfers = state.allTransfers
        .where((t) => t.isInProgress || t.isOffered)
        .length;

    if (_isWide) {
      return _wideLayout(state, pendingTransfers);
    }
    return _narrowLayout(state, pendingTransfers);
  }

  // ---------- 宽屏：左侧导航 ----------

  Widget _wideLayout(AppState state, int pendingTransfers) {
    final cc = LunoteColors.of(context);
    final hasDevices = state.peers.values.any((p) => p.online);
    return Row(
      children: [
        Container(
          width: 250,
          decoration: BoxDecoration(
            color: cc.nightRaised,
            border: Border(right: BorderSide(color: cc.nightSoft, width: 1)),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const SizedBox(height: 26),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 22),
                child: Row(
                  children: [
                    const LunoteMark(size: 40),
                    const SizedBox(width: 12),
                    Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          '月笺',
                          style: TextStyle(
                            fontSize: 17,
                            fontWeight: FontWeight.w800,
                            color: cc.moon,
                            letterSpacing: 2,
                          ),
                        ),
                        Text(
                          'LUNOTE',
                          style: TextStyle(
                            fontSize: 10,
                            letterSpacing: 3,
                            color: cc.moonDim,
                          ),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 26),
              _navItem(
                _Nav.devices,
                Icons.devices_rounded,
                '设备',
                badge: hasDevices ? null : null,
              ),
              _navItem(
                _Nav.conversations,
                Icons.chat_bubble_rounded,
                '对话',
                badge: state.conversationOrder.isEmpty ? null : null,
              ),
              _navItem(
                _Nav.transfers,
                Icons.swap_vert_rounded,
                '传输',
                badge: pendingTransfers > 0 ? pendingTransfers : null,
              ),
              _navItem(_Nav.settings, Icons.settings_rounded, '设置'),
              const Spacer(),
              _myDeviceCard(state),
            ],
          ),
        ),
        Expanded(
          child: switch (_nav) {
            _Nav.devices => DevicesPage(onOpenConversation: _openConversation),
            _Nav.conversations => _conversationsPage(state),
            _Nav.transfers => const TransfersPage(),
            _Nav.settings => const SettingsPage(),
          },
        ),
      ],
    );
  }

  // ---------- 窄屏：底部导航 ----------

  Widget _narrowLayout(AppState state, int pendingTransfers) {
    final cc = LunoteColors.of(context);
    return Scaffold(
      body: SafeArea(
        child: switch (_nav) {
          _Nav.devices => DevicesPage(onOpenConversation: _openConversation),
          _Nav.conversations => _conversationsPage(state),
          _Nav.transfers => const TransfersPage(),
          _Nav.settings => const SettingsPage(),
        },
      ),
      bottomNavigationBar: NavigationBar(
        backgroundColor: cc.nightRaised,
        indicatorColor: cc.nightSoft,
        selectedIndex: _nav.index,
        onDestinationSelected: (i) => setState(() => _nav = _Nav.values[i]),
        destinations: [
          NavigationDestination(
            icon: Icon(Icons.devices_rounded, color: cc.moonDim),
            selectedIcon: Icon(Icons.devices_rounded, color: cc.gold),
            label: '设备',
          ),
          NavigationDestination(
            icon: Icon(Icons.chat_bubble_rounded, color: cc.moonDim),
            selectedIcon: Icon(Icons.chat_bubble_rounded, color: cc.gold),
            label: '对话',
          ),
          NavigationDestination(
            icon: Badge(
              isLabelVisible: pendingTransfers > 0,
              label: Text('$pendingTransfers'),
              child: Icon(Icons.swap_vert_rounded, color: cc.moonDim),
            ),
            selectedIcon: Badge(
              isLabelVisible: pendingTransfers > 0,
              label: Text('$pendingTransfers'),
              child: Icon(Icons.swap_vert_rounded, color: cc.gold),
            ),
            label: '传输',
          ),
          NavigationDestination(
            icon: Icon(Icons.settings_rounded, color: cc.moonDim),
            selectedIcon: Icon(Icons.settings_rounded, color: cc.gold),
            label: '设置',
          ),
        ],
      ),
    );
  }

  // ---------- 对话 ----------

  Widget _conversationsPage(AppState state) {
    final cc = LunoteColors.of(context);
    final order = state.conversationOrder;
    final deviceId = _activeDevice;
    if (_isWide && deviceId != null) {
      // 宽屏内嵌：常驻返回按钮（onBack 回到对话列表）；
      // 不要求设备在线/在发现列表——历史对话也应能进入查看
      return ChatPage(
        key: ValueKey(deviceId),
        deviceId: deviceId,
        onBack: () => setState(() => _activeDevice = null),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: EdgeInsets.fromLTRB(
            _isWide ? 24 : 16,
            _isWide ? 12 : 4,
            12,
            4,
          ),
          child: SizedBox(
            height: 48,
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    _selectingConversations
                        ? '已选择 ${_selectedConversations.length} 项'
                        : '对话',
                    style: TextStyle(
                      fontSize: _selectingConversations ? 17 : 22,
                      fontWeight: FontWeight.w700,
                      color: cc.moon,
                    ),
                  ),
                ),
                if (_selectingConversations) ...[
                  IconButton(
                    onPressed: () => setState(() {
                      if (_selectedConversations.length == order.length) {
                        _selectedConversations.clear();
                      } else {
                        _selectedConversations
                          ..clear()
                          ..addAll(order);
                      }
                    }),
                    tooltip: _selectedConversations.length == order.length
                        ? '取消全选'
                        : '全选',
                    icon: Icon(
                      _selectedConversations.length == order.length
                          ? Icons.deselect_rounded
                          : Icons.select_all_rounded,
                      color: cc.moon,
                    ),
                  ),
                  IconButton(
                    onPressed: () => _confirmDeleteConversations(
                      context,
                      state,
                      _selectedConversations,
                    ),
                    tooltip: '删除所选对话',
                    icon: Icon(Icons.delete_rounded, color: cc.warn),
                  ),
                  IconButton(
                    onPressed: () => setState(() {
                      _conversationSelectionMode = false;
                      _selectedConversations.clear();
                    }),
                    tooltip: '退出选择',
                    icon: Icon(Icons.close_rounded, color: cc.moonDim),
                  ),
                ] else if (order.isNotEmpty)
                  IconButton(
                    onPressed: () => setState(() {
                      _conversationSelectionMode = true;
                      _selectedConversations.clear();
                    }),
                    tooltip: '批量管理对话',
                    icon: Icon(Icons.checklist_rounded, color: cc.gold),
                  ),
              ],
            ),
          ),
        ),
        Expanded(
          child: order.isEmpty
              ? Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.forum_rounded, size: 44, color: cc.nightSoft),
                      SizedBox(height: 12),
                      Text('还没有对话', style: TextStyle(color: cc.moonDim)),
                    ],
                  ),
                )
              : ListView.builder(
                  padding: EdgeInsets.fromLTRB(
                    _isWide ? 18 : 10,
                    6,
                    _isWide ? 18 : 10,
                    18,
                  ),
                  itemCount: order.length,
                  itemBuilder: (context, i) {
                    final id = order[i];
                    final name = state.peerName(id);
                    final msgs = state.messagesOf(id);
                    final last = msgs.isEmpty ? null : msgs.last;
                    final selected = _selectedConversations.contains(id);
                    // 长按（移动端）/右键（PC）进入批量选择。
                    return GestureDetector(
                      onSecondaryTapDown: (_) => _toggleConversation(id),
                      child: SpringButton(
                        weight: SpringWeight.normal,
                        onLongPress: () => _toggleConversation(id),
                        onTap: () {
                          if (_selectingConversations) {
                            _toggleConversation(id);
                            return;
                          }
                          _openConversation(id);
                        },
                        child: Container(
                          margin: const EdgeInsets.symmetric(vertical: 4),
                          padding: const EdgeInsets.all(13),
                          decoration: BoxDecoration(
                            color: selected ? cc.nightSoft : cc.nightRaised,
                            borderRadius: BorderRadius.circular(14),
                            border: selected
                                ? Border.all(color: cc.gold, width: 1.5)
                                : null,
                          ),
                          child: Row(
                            children: [
                              Container(
                                width: 36,
                                height: 36,
                                decoration: BoxDecoration(
                                  color: cc.nightSoft,
                                  shape: BoxShape.circle,
                                ),
                                alignment: Alignment.center,
                                child: Text(
                                  name.isEmpty
                                      ? '?'
                                      : String.fromCharCode(name.runes.first)
                                            .toUpperCase(),
                                  style: TextStyle(
                                    color: cc.gold,
                                    fontWeight: FontWeight.w700,
                                  ),
                                ),
                              ),
                              const SizedBox(width: 10),
                              Expanded(
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text(
                                      name,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: TextStyle(
                                        fontSize: 13.5,
                                        fontWeight: FontWeight.w600,
                                        color: cc.moon,
                                      ),
                                    ),
                                    const SizedBox(height: 2),
                                    Text(
                                      last?.text ?? '（暂无消息）',
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: TextStyle(
                                        fontSize: 11.5,
                                        color: cc.moonDim,
                                      ),
                                    ),
                                  ],
                                ),
                              ),
                              if (selected)
                                Padding(
                                  padding: const EdgeInsets.only(left: 10),
                                  child: Icon(
                                    Icons.check_circle_rounded,
                                    size: 21,
                                    color: cc.gold,
                                  ),
                                ),
                            ],
                          ),
                        ),
                      ),
                    );
                  },
                ),
        ),
      ],
    );
  }

  void _toggleConversation(String id) {
    setState(() {
      _conversationSelectionMode = true;
      if (!_selectedConversations.add(id)) {
        _selectedConversations.remove(id);
      }
    });
  }

  void _openConversation(String id) {
    setState(() {
      _nav = _Nav.conversations;
      _conversationSelectionMode = false;
      _selectedConversations.clear();
      if (_isWide) _activeDevice = id;
    });
    if (_isWide) return;
    final cc = LunoteColors.of(context);
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => Scaffold(
          backgroundColor: cc.night,
          body: SafeArea(child: ChatPage(deviceId: id)),
        ),
      ),
    );
  }

  /// 批量删除本地消息与传输记录；信任关系和设备身份保留。
  Future<void> _confirmDeleteConversations(
    BuildContext context,
    AppState state,
    Iterable<String> selected,
  ) async {
    final ids = selected.toSet().toList();
    if (ids.isEmpty) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) {
        final c2 = LunoteColors.of(ctx);
        return AlertDialog(
          title: Text(ids.length == 1 ? '删除对话？' : '删除 ${ids.length} 个对话？'),
          content: Text(
            ids.length == 1
                ? '将删除与「${state.peerName(ids.first)}」的本地消息与传输记录（不可恢复）。\n信任关系与设备身份保留。'
                : '将一次性删除所选对话的本地消息与传输记录（不可恢复）。\n信任关系与设备身份保留。',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('取消'),
            ),
            TextButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: Text('删除', style: TextStyle(color: c2.warn)),
            ),
          ],
        );
      },
    );
    if (ok == true) {
      final err = await state.deleteConversations(ids);
      if (err != null && context.mounted) {
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('删除失败：$err')));
      } else if (context.mounted) {
        setState(() {
          _conversationSelectionMode = false;
          _selectedConversations.clear();
          if (ids.contains(_activeDevice)) _activeDevice = null;
        });
        ScaffoldMessenger.of(context)
            .showSnackBar(SnackBar(content: Text('已删除 ${ids.length} 个对话')));
      }
    }
  }

  Widget _myDeviceCard(AppState state) {
    final cc = LunoteColors.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(
          color: cc.night,
          borderRadius: BorderRadius.circular(16),
        ),
        child: Row(
          children: [
            Container(
              width: 30,
              height: 30,
              decoration: BoxDecoration(
                color: cc.nightSoft,
                shape: BoxShape.circle,
              ),
              child: Icon(Icons.person_rounded, size: 16, color: cc.gold),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    state.deviceName,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: cc.moon,
                    ),
                  ),
                  Text(
                    state.deviceId.isEmpty
                        ? '启动中…'
                        : state.deviceId.substring(0, 8),
                    style: TextStyle(fontSize: 10.5, color: cc.moonDim),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _navItem(_Nav nav, IconData icon, String label, {int? badge}) {
    final cc = LunoteColors.of(context);
    final active = _nav == nav;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 2),
      child: SpringButton(
        weight: SpringWeight.normal,
        onTap: () => setState(() => _nav = nav),
        borderRadius: 14,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOutCubic,
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
          decoration: BoxDecoration(
            color: active ? cc.nightSoft : Colors.transparent,
            borderRadius: BorderRadius.circular(14),
          ),
          child: Row(
            children: [
              Icon(icon, size: 19, color: active ? cc.gold : cc.moonDim),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  label,
                  style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? cc.moon : cc.moonDim,
                  ),
                ),
              ),
              if (badge != null)
                Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: 7,
                    vertical: 2,
                  ),
                  decoration: BoxDecoration(
                    color: cc.gold,
                    borderRadius: BorderRadius.all(Radius.circular(10)),
                  ),
                  child: Text(
                    '$badge',
                    style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w800,
                      color: cc.night,
                    ),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
