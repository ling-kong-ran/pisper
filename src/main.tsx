// 先恢复跨随机端口的本机偏好，再加载会在模块初始化时读取持久状态的应用 Store。
import './index.css'
import { restorePageState } from '@/lib/storage/page-state-storage'

void restorePageState().then(() => import('@/mount-app'))
