type DeviceNavigator = Pick<Navigator, 'userAgent' | 'platform' | 'maxTouchPoints'>

// 触屏电脑同样具备 coarse pointer，不能据此启用手机导航。
// iPad 的桌面浏览模式使用 Mac 标识，需要结合多点触控识别。
export function isMobileDevice(device: DeviceNavigator): boolean {
  if (/Android|iPhone|iPad|iPod/i.test(device.userAgent)) return true
  return (
    (device.platform === 'MacIntel' || /Macintosh/i.test(device.userAgent)) &&
    device.maxTouchPoints > 1
  )
}
