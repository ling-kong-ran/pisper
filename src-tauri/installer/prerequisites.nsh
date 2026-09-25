; 此区段在 Tauri 的 WebView2 区段之前运行，不接管上游安装、签名和退出码处理。
; 使用系统语言；此时 MUI 尚未声明语言常量，1033/2052 为 NSIS 语言 ID。
!ifdef PISPER_OFFLINE_INSTALLER
LangString pisperMissingWebView2 1033 "Microsoft WebView2 Runtime is missing. Pisper needs it to display its interface.$\r$\n$\r$\nThis offline package includes the dependency and will install it locally. No speech model is being downloaded."
LangString pisperMissingWebView2 2052 "检测到缺少 Microsoft WebView2 运行时，Pisper 需要它来显示界面。$\r$\n$\r$\n当前为完整离线包，将使用内置依赖安装，无需联网，也不会下载语音模型。"
!else
LangString pisperMissingWebView2 1033 "Microsoft WebView2 Runtime is missing. Pisper needs it to display its interface.$\r$\n$\r$\nThis standard package will download and install it from Microsoft. No speech model is being downloaded.$\r$\n$\r$\nFor an offline PC, cancel and get the full offline package (*-offline-setup.exe) from https://github.com/ling-kong-ran/pisper/releases/latest on another PC."
LangString pisperMissingWebView2 2052 "检测到缺少 Microsoft WebView2 运行时，Pisper 需要它来显示界面。$\r$\n$\r$\n当前为普通包，将联网从微软下载并安装此依赖，不是下载语音模型。$\r$\n$\r$\n如果本机无法联网，请取消，在其他电脑从 https://github.com/ling-kong-ran/pisper/releases/latest 下载完整离线包（*-offline-setup.exe）后拷贝安装。"
!endif

Section "-Pisper prerequisites"
  Push $0
  Push $1
  ${If} ${RunningX64}
    ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${Else}
    ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    ReadRegStr $0 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    SetDetailsView show
    DetailPrint "$(pisperMissingWebView2)"
    ; 无人值守模式保留安装详情，不弹出需要人工确认的窗口。
    ${GetOptions} $CMDLINE "/P" $1
    ${If} ${Errors}
      IfSilent pisper_prerequisites_done
      MessageBox MB_OKCANCEL|MB_ICONINFORMATION "$(pisperMissingWebView2)" IDOK pisper_prerequisites_done
      Pop $1
      Pop $0
      Abort
    ${EndIf}
  ${EndIf}
  pisper_prerequisites_done:
  Pop $1
  Pop $0
SectionEnd
