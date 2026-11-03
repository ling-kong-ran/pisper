// 新建项目：选目录 → 登记为项目并选为启动台的派发目标。系统目录对话框不可用时退回 WorkspacePicker
// 手动填写；也可以就地新建一个空文件夹（NewFolderDialog → Runtime 建目录）再登记。
// 侧栏 / 看板的项目菜单（经 useProjectCommands）与启动台项目芯片共用这一条流程，
// 各自只决定选完之后焦点交给谁、错误怎么提示。
import { useCallback, useEffect, useRef, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { pickProjectDirectory } from '@/features/chat/project-actions'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { parentDirectory } from './folder-paths'
import { useLaunchDefaultsStore } from './launch-defaults-store'

export type NewProjectPickerOptions = {
  // 新目录已经选为派发目标之后（经手动填写对话框时等它关闭、焦点归还阶段再调用）。
  onProjectChosen: () => void
  onError: (message: string) => void
  // 登记完成、焦点还没交出去时：调用方可以用提示告知选中的项目。
  onRemembered?: (path: string) => void
}

export function useNewProjectPicker({
  onProjectChosen,
  onError,
  onRemembered,
}: NewProjectPickerOptions) {
  const { t } = useI18n()
  const [picking, setPicking] = useState(false)
  const [pickerMounted, setPickerMounted] = useState(false)
  const [pickerOpen, setPickerOpen] = useState(false)
  const [folderMounted, setFolderMounted] = useState(false)
  const [folderOpen, setFolderOpen] = useState(false)
  const [folderParent, setFolderParent] = useState('')
  const pickingRef = useRef(false)
  const requestRef = useRef(0)
  // 经手动填写或新建文件夹对话框选好目录后，等对话框关闭、焦点归还阶段再交出焦点，否则会被对话框抢回去。
  const focusAfterPickerRef = useRef(false)
  const callbacksRef = useRef({ onProjectChosen, onError, onRemembered })
  callbacksRef.current = { onProjectChosen, onError, onRemembered }

  // 原生对话框可能在调用方卸载后才返回：旧结果不能再改偏好或弹提示。
  useEffect(
    () => () => {
      requestRef.current += 1
    },
    [],
  )

  const remember = useCallback((picked: string) => {
    // 先登记：还没有会话的新目录也要出现在最近项目、启动台、项目筛选与管理项目里。
    // 派发目标用登记后的规范化目录，免得同一目录因末尾斜杠不同在启动台里出现两次。
    const path = useRegisteredProjectsStore.getState().registerProject(picked) || picked
    useLaunchDefaultsStore.getState().rememberLaunch({ cwd: path })
    callbacksRef.current.onRemembered?.(path)
  }, [])

  const newProject = useCallback(async () => {
    if (pickingRef.current) return
    pickingRef.current = true
    setPicking(true)
    const request = ++requestRef.current
    try {
      const result = await pickProjectDirectory(t)
      if (request !== requestRef.current) return
      if (result.status === 'picked') {
        remember(result.path)
        callbacksRef.current.onProjectChosen()
      } else if (result.status === 'unavailable') {
        setPickerMounted(true)
        setPickerOpen(true)
      }
    } catch (error) {
      if (request === requestRef.current)
        callbacksRef.current.onError(error instanceof Error ? error.message : String(error))
    } finally {
      if (request === requestRef.current) {
        pickingRef.current = false
        setPicking(false)
      }
    }
  }, [remember, t])

  // 新建文件夹：位置预填为 near（通常是当前项目）的上级目录，新文件夹与它并列；没有时交给 Runtime 默认目录。
  const newFolder = useCallback((near = '') => {
    setFolderParent(parentDirectory(near))
    setFolderMounted(true)
    setFolderOpen(true)
  }, [])

  const handOffFocus = (event: Event) => {
    if (!focusAfterPickerRef.current) return
    focusAfterPickerRef.current = false
    event.preventDefault()
    callbacksRef.current.onProjectChosen()
  }

  // 系统目录对话框不可用时退回的手动填写对话框（WorkspacePicker）的属性；由调用方按需加载并渲染。
  const picker = {
    mounted: pickerMounted,
    open: pickerOpen,
    description: t('board:project.pickerDescription'),
    onOpenChange: setPickerOpen,
    onSelect: (path: string) => {
      remember(path)
      focusAfterPickerRef.current = true
    },
    onCloseAutoFocus: handOffFocus,
  }

  // 新建文件夹对话框（NewFolderDialog）的属性；同样由调用方按需加载并渲染。
  const folderDialog = {
    mounted: folderMounted,
    open: folderOpen,
    initialParent: folderParent,
    onOpenChange: setFolderOpen,
    onCreated: (path: string) => {
      remember(path)
      focusAfterPickerRef.current = true
    },
    onCloseAutoFocus: handOffFocus,
  }

  return { newProject, newFolder, picking, picker, folderDialog }
}
