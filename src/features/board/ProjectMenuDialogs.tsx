// 项目菜单（useProjectMenu）用到的对话框：新建项目的选目录器与新建文件夹（复用 ProjectPickerDialogs），
// 以及管理项目；都按需懒加载，第一次打开前不进包也不挂载。
import { lazy, Suspense } from 'react'
import { ProjectPickerDialogs } from './ProjectPickerDialogs'
import type { ProjectMenuDialogsProps } from './useProjectMenu'

const ManageProjectsDialog = lazy(() =>
  import('./ManageProjectsDialog').then((module) => ({ default: module.ManageProjectsDialog })),
)

export function ProjectMenuDialogs({
  commands,
  manageMounted,
  manageOpen,
  onManageOpenChange,
}: ProjectMenuDialogsProps) {
  return (
    <>
      <ProjectPickerDialogs picker={commands.picker} folderDialog={commands.folderDialog} />
      {manageMounted && (
        <Suspense fallback={null}>
          <ManageProjectsDialog
            open={manageOpen}
            onOpenChange={onManageOpenChange}
            commands={commands}
          />
        </Suspense>
      )}
    </>
  )
}
