// useNewProjectPicker 的两个对话框（手动填写目录、新建文件夹）：按需加载，打开过才挂载。
import { lazy, Suspense } from 'react'
import type { useNewProjectPicker } from './useNewProjectPicker'

const WorkspacePicker = lazy(() =>
  import('@/components/WorkspacePicker').then((module) => ({ default: module.WorkspacePicker })),
)
const NewFolderDialog = lazy(() =>
  import('./NewFolderDialog').then((module) => ({ default: module.NewFolderDialog })),
)

type ProjectPicker = ReturnType<typeof useNewProjectPicker>

export function ProjectPickerDialogs({
  picker,
  folderDialog,
}: Pick<ProjectPicker, 'picker' | 'folderDialog'>) {
  const { mounted: pickerMounted, ...pickerProps } = picker
  const { mounted: folderDialogMounted, ...folderDialogProps } = folderDialog
  if (!pickerMounted && !folderDialogMounted) return null
  return (
    <Suspense fallback={null}>
      {pickerMounted && <WorkspacePicker {...pickerProps} />}
      {folderDialogMounted && <NewFolderDialog {...folderDialogProps} />}
    </Suspense>
  )
}
