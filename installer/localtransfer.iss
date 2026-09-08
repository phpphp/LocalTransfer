; LocalTransfer Inno Setup 安装脚本
; 编译: ISCC.exe localtransfer.iss（由 scripts\make-installer.ps1 调用）
; 说明：per-user 安装（免 UAC），装到 %LOCALAPPDATA%\Programs\LocalTransfer；
;       旧版本检测/覆盖升级由 Inno 按 AppId+Version 自动处理。

#define MyAppName "LocalTransfer"
#define MyAppVersion "0.2.0"
#define MyAppExeName "local-transfer.exe"
#define MyAppPublisher "LocalTransfer"
#define MyAppDescription "LocalTransfer - LAN file & text transfer"

[Setup]
; 固定 AppId（不要改，升级识别靠它）
AppId={{8B7E3B6A-63D1-4C9B-9E24-9A0E5C71F2A3}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; 免 UAC：以当前用户安装
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=LocalTransfer-Setup-{#MyAppVersion}-win64
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
; 安装前关闭正在运行的程序（PrepareToInstall 里杀进程）
CloseApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

; 界面关键文案汉化（覆盖英文默认）
[Messages]
en.WelcomeLabel2=这将安装 [name/ver] 到您的电脑。%n%n建议关闭其他应用程序后继续。
en.SelectDirDesc=选择 [name] 的安装位置。
en.SelectDirLabel3=安装程序将把 [name] 安装到以下文件夹。
en.SelectDirBrowseLabel=点击"下一步"继续。如需换一个文件夹，点击"浏览"。
en.DirExistsTitle=文件夹已存在
en.DirExists=文件夹已存在。可以直接安装到该文件夹，也可以换一个。
en.ReadyLabel1=安装程序已准备好开始安装 [name]。
en.ReadyLabel2a=点击"安装"开始，点击"上一步"检查或修改设置。
en.InstallingLabel=正在安装，请稍候…
en.FinishedHeadingLabel=安装完成
en.FinishedLabelNoIcons=[name] 已安装到您的电脑。
en.FinishedLabel=勾选下面的选项运行 [name]。
en.RunEntryNow=现在运行
en.ExitSetupTitle=退出安装
en.ExitSetupMessage=安装尚未完成。如果现在退出，程序将不会被安装。%n%n可以稍后再次运行安装程序完成安装。退出安装吗？
en.WelcomeLabel1=欢迎使用 [name] 安装向导
en.SelectDirDesc3=
en.SelectTasksLabel2=选择安装程序要执行的附加任务。

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式(&D)"; GroupDescription: "附加快捷方式："
Name: "startmenuicon"; Description: "创建开始菜单快捷方式(&S)"; GroupDescription: "附加快捷方式:"

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\README.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\scripts\add-firewall-rule.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\scripts\install.ps1"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: startmenuicon
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"; Tasks: startmenuicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "运行 {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 只删程序文件，%APPDATA% 的配置与聊天记录保留

[Code]
// 安装前关闭正在运行的实例（否则文件被占用换不出新 exe）
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := '';
  Exec('taskkill.exe', '/IM local-transfer.exe /F', '', SW_HIDE,
       ewWaitUntilTerminated, ResultCode);
end;
