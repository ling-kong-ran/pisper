; 安装、升级和卸载都不留下旧东西。
; Tauri 的 NSIS 升级（包括自动更新传入的 /UPDATE）只覆盖文件、从不删除：旧版本带过、
; 新版本已经不带的文件会一直留在安装目录。sidecar-runtime 里残留的旧 node_modules
; 会被 Node 优先解析，Agent 直接起不来，所以每次安装前把整套随包目录清掉再放新的。
; 本文件在模板定义 BUNDLEID、INSTALLMODE 等常量之前被包含：用到它们的地方只能写在宏里
; （插入时才展开），函数只通过栈接收路径。

; 随安装包整套提供的目录（tauri.conf.json 的 bundle.resources）。只删这几个，
; 不递归删 $INSTDIR：用户可能把程序装进了放着别的东西的目录。
!macro PisperClearBundledDirs
  ${If} $INSTDIR != ""
    RMDir /r "$INSTDIR\sidecar-runtime"
    RMDir /r "$INSTDIR\desktop"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; 先确认程序已经退出（与模板随后的检查同一个提示）。用户在这里取消会中止安装，
  ; 此时还什么都没删，原来的程序照常可用。
  !insertmacro CheckIfAppIsRunning "${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro PisperClearBundledDirs
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 升级时模板不会走卸载；万一带着 /UPDATE 卸载，紧接着就要装新版本，不动用户环境。
  ${If} $UpdateMode <> 1
    !insertmacro PisperClearBundledDirs
    RMDir "$INSTDIR"

    SetShellVarContext current
    ; 设置里“安装命令行工具”放的 pisper.exe 与 PATH 条目。只处理带托管标记的，
    ; 与桌面端 desktop_uninstall_cli 相同：用户自己放的同名程序不碰。
    ${If} ${FileExists} "$LOCALAPPDATA\${BUNDLEID}\cli\.pisper-cli-managed"
      Delete /REBOOTOK "$LOCALAPPDATA\${BUNDLEID}\cli\pisper.exe"
      Delete "$LOCALAPPDATA\${BUNDLEID}\cli\.pisper-cli-managed"
      RMDir "$LOCALAPPDATA\${BUNDLEID}\cli"
      Push "$LOCALAPPDATA\${BUNDLEID}\cli"
      Call un.PisperRemoveUserPathEntry
    ${EndIf}
    ; 下载的组件更新也是程序本身。npm 版 pisper 共用这个目录，但它每次启动都会检查，
    ; 缺了会自己重新下载。
    RMDir /r "$LOCALAPPDATA\${BUNDLEID}\components"
    Delete "$LOCALAPPDATA\${BUNDLEID}\desktop-sidecar.json"
    Delete "$LOCALAPPDATA\${BUNDLEID}\iroh-tunnel-status.json"

    ; 会话、设置和记忆在 ~/.pisper；勾选“删除应用程序数据”时一并删除
    ; （模板已经删了两个应用数据目录，里面有内置浏览器的登录状态）。
    ${If} $DeleteAppDataCheckboxState = 1
      RMDir /r "$PROFILE\.pisper"
    ${EndIf}

    ; 没勾选时保留登录状态等数据；目录已经空了才删。
    RMDir "$LOCALAPPDATA\${BUNDLEID}"
    RMDir "$APPDATA\${BUNDLEID}"
  ${EndIf}
!macroend

; 与 Rust 的 normalized 一致：去掉首尾空格和引号、末尾的斜杠；比较时 NSIS 本身不分大小写。
Function un.PisperNormalizePath
  Exch $0
  Push $1
  lead:
    StrCpy $1 $0 1
    ${If} $1 == " "
    ${OrIf} $1 == '"'
      StrCpy $0 $0 "" 1
      Goto lead
    ${EndIf}
  trail:
    StrCpy $1 $0 "" -1
    ${If} $1 == " "
    ${OrIf} $1 == '"'
    ${OrIf} $1 == "\"
    ${OrIf} $1 == "/"
      StrCpy $0 $0 -1
      Goto trail
    ${EndIf}
  Pop $1
  Exch $0
FunctionEnd

; 从用户 PATH 去掉一个目录，其余条目（包括空条目）原样保留。栈顶是要去掉的目录。
Function un.PisperRemoveUserPathEntry
  Exch $R9
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7

  ClearErrors
  ReadRegStr $0 HKCU "Environment" "Path"
  ; 读不到，或长度到了 NSIS 字符串上限（可能已经截断）：宁可留下一个失效条目，
  ; 也不能把半截 PATH 写回去。
  IfErrors done
  StrLen $7 $0
  IntCmp $7 0 done
  IntOp $6 ${NSIS_MAX_STRLEN} - 2
  IntCmp $7 $6 done 0 done

  Push $R9
  Call un.PisperNormalizePath
  Pop $R9
  StrCmp $R9 "" done

  StrCpy $1 "" ; 保留下来的 PATH
  StrCpy $2 "" ; 当前条目
  StrCpy $3 0  ; 读到第几个字符
  StrCpy $4 0  ; 已保留的条目数
  StrCpy $5 0  ; 是否去掉过
  loop:
    StrCpy $6 $0 1 $3
    ${If} $6 == ";"
    ${OrIf} $6 == ""
      Push $2
      Call un.PisperNormalizePath
      Pop $7
      ${If} $7 == $R9
        StrCpy $5 1
      ${ElseIf} $4 = 0
        StrCpy $1 $2
        IntOp $4 $4 + 1
      ${Else}
        StrCpy $1 "$1;$2"
        IntOp $4 $4 + 1
      ${EndIf}
      StrCpy $2 ""
      StrCmp $6 "" finish
    ${Else}
      StrCpy $2 "$2$6"
    ${EndIf}
    IntOp $3 $3 + 1
    Goto loop

  finish:
  ${If} $5 = 1
    WriteRegExpandStr HKCU "Environment" "Path" $1
    ; 通知已经打开的程序 PATH 变了（新开的终端立刻生效）。
    SendMessage 0xFFFF 0x001A 0 "STR:Environment" /TIMEOUT=2000
  ${EndIf}

  done:
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
  Pop $R9
FunctionEnd
