LocalTransfer v0.2.0 — 局域网文件/文本传输（Linux x64）
==========================================================

【解压与运行】
    tar xzf LocalTransfer-0.2.0-x86_64-unknown-linux-gnu.tar.gz
    cd LocalTransfer-0.2.0-x86_64-unknown-linux-gnu

先装系统依赖（Ubuntu/Debian 系，gpui 图形栈需要）：

    sudo apt install libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
        libgl1 libegl1 libasound2t64 libfontconfig1 libfreetype6 \
        libxi6 libxcursor1 libxrandr2 libxinerama1

运行：

    ./local-transfer

（Ubuntu 24.04 之前的系统，把 libasound2t64 换成 libasound2。）

【加入应用菜单（可选）】
把 desktop 文件与图标装进系统，之后可从应用菜单启动：

    # 假设解压目录为 ~/LocalTransfer-0.2.0-x86_64-unknown-linux-gnu
    mkdir -p ~/.local/share/applications ~/.local/icons
    cp localtransfer.desktop ~/.local/share/applications/
    cp icon.png ~/.local/icons/localtransfer.png
    # 编辑 ~/.local/share/applications/localtransfer.desktop，
    # 把 Exec= 与 Icon= 改成解压目录的绝对路径，例如：
    #   Exec=/home/你的用户名/LocalTransfer-0.2.0-x86_64-unknown-linux-gnu/local-transfer
    #   Icon=/home/你的用户名/.local/icons/localtransfer.png
    update-desktop-database ~/.local/share/applications 2>/dev/null || true

或者直接把程序放进 PATH（更省事）：

    sudo cp local-transfer /usr/local/bin/
    sudo cp icon.png /usr/local/share/icons/localtransfer.png
    sudo sed -i 's|Exec=.*|Exec=/usr/local/bin/local-transfer|; s|Icon=.*|Icon=/usr/local/share/icons/localtransfer.png|' \
        localtransfer.desktop
    sudo cp localtransfer.desktop /usr/share/applications/

【使用】
同一局域网内的设备会自动互相发现（UDP 多播 + 广播）。
找不到对方时，点"添加设备"输入对方 IP 直连，或扫对端二维码。

【接收文件】
默认保存到「下载/LocalTransfer」目录，可在设置中修改；
接收时可点"存到…"选择本次保存位置。设置里可开启"自动接收文件"。

【防火墙（重要）】
发现不了设备时，通常是防火墙拦了多播/广播端口（UDP 17878）：

    sudo ufw allow 17878/udp
    sudo ufw allow 17878/tcp

【数据位置】
配置与历史记录保存在 ~/.config/LocalTransfer/

【已知约定】
默认端口 17878（可在设置中修改，重启生效）。仅单个实例运行：
二次启动会把已运行实例的窗口拉到前台。
