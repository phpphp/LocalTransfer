LocalTransfer v0.2.0 — 局域网文件/文本传输（Linux x64）
==========================================================

【快速开始】

    # 1. 安装依赖（Ubuntu/Debian，只需一次）
    sudo apt install libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
        libgl1 libegl1 libasound2t64 libfontconfig1 libfreetype6 \
        libxi6 libxcursor1 libxrandr2 libxinerama1

    # 2. 运行
    ./local-transfer

    # 3. 防火墙（发现不了设备时）
    sudo ufw allow 17878/udp && sudo ufw allow 17878/tcp

> Ubuntu 24.04 之前把 libasound2t64 换成 libasound2。

【推荐：用 .run 安装器】

同级目录的 LocalTransfer-Setup-0.2.0-linux-x64.run 一条命令装好
（程序 + 应用菜单图标 + 终端命令）：

    chmod +x LocalTransfer-Setup-0.2.0-linux-x64.run
    ./LocalTransfer-Setup-0.2.0-linux-x64.run

安装后：终端运行 local-transfer，或从应用菜单「网络」分类启动。

【手动加入应用菜单（可选）】

    sudo cp local-transfer /usr/local/bin/
    sudo cp icon.png /usr/local/share/icons/hicolor/512x512/apps/localtransfer.png
    sudo sed -e 's|Exec=.*|Exec=/usr/local/bin/local-transfer|' \
             -e 's|Icon=.*|Icon=/usr/local/share/icons/hicolor/512x512/apps/localtransfer.png|' \
             localtransfer.desktop | sudo tee /usr/share/applications/localtransfer.desktop

【使用】

- 同一局域网设备自动互相发现；找不到时点「添加设备」输入 IP
- 接收文件默认存「下载/LocalTransfer」，可用「存到…」自选位置
- 聊天头部可测速（对端传输速度 + 本机 WiFi 链路质量）
- 数据（配置/历史）在 ~/.config/LocalTransfer/
