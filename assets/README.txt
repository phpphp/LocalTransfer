LocalTransfer v0.1.0 — 局域网文件/文本传输
============================================

【使用】
双击 local-transfer.exe 即可运行，无需安装。
首次启动会自动生成随机设备名（可在设置中修改）。

同一局域网内的设备会自动互相发现（UDP 多播 + 广播）。
找不到对方时，可点"添加设备"输入对方 IP 直连。

【接收文件】
默认保存到「下载\LocalTransfer」目录，可在设置中修改。
设置里可开启"自动接收文件"（跳过确认）。

【防火墙（重要）】
首次运行 Windows 可能弹出防火墙提示，请勾选"专用网络"允许。
若没弹窗且发现不了设备，以管理员身份运行 PowerShell 执行：

    powershell -ExecutionPolicy Bypass -File add-firewall-rule.ps1

【数据位置】
配置与历史记录保存在 %APPDATA%\LocalTransfer\

【已知约定】
默认端口 17878（可在设置中修改，重启生效）。
