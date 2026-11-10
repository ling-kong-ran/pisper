// 群聊消息的附件：发消息时把内容归档成资产（和会话附件同一个资产库，同内容只存一份），消息里只留引用；
// 叫醒 Bot 时按引用读回内容，交给席位会话走正常的附件流程；撤回时只删“只有这条消息用到”的资产。
// 资产库的读写由调用方注入（运行时的 createAsset / getAssetDownload / findAsset / deleteAsset），这里不碰文件。
import { isPisperError, pisperError } from '../../../shared/errors.mjs'
import {
  MAX_ROOM_ATTACHMENTS,
  MAX_ROOM_ATTACHMENTS_BYTES,
  roomAttachmentRefs,
} from './room-model.mjs'

// 群里上传的附件在资产库里的来源：和会话附件一样记作 attachment，不挂在哪条会话上（sessionId 为空）。
const ROOM_ASSET_SOURCE = 'attachment'

/**
 * 把校验过的附件归档成资产，返回消息里存的引用。本地路径不归档，只存路径。
 * 有一个归档失败就整条不发：已经存下的、只属于这次的资产删掉，不留孤儿。
 * @param {{
 *   attachments: Record<string, any>[],
 *   roomName: string,
 *   createAsset: (input: Record<string, any>) => Promise<{ id: string } | null>,
 *   release: (assetId: string) => Promise<unknown>,
 * }} input
 */
export async function archiveRoomAttachments({ attachments, roomName, createAsset, release }) {
  const refs = []
  try {
    for (const attachment of attachments) {
      if (attachment.kind === 'path') {
        refs.push({ kind: 'path', name: attachment.name, path: attachment.path })
        continue
      }
      const asset = await createAsset({
        name: attachment.name,
        mimeType: attachment.mimeType,
        ...(attachment.kind === 'text' ? { text: attachment.text } : { data: attachment.data }),
        source: ROOM_ASSET_SOURCE,
        sessionId: '',
        sessionName: roomName,
      })
      if (!asset?.id) throw pisperError('failed', 'attachment', attachment.name)
      refs.push({
        id: asset.id,
        kind: attachment.kind,
        name: attachment.name,
        mimeType: attachment.mimeType,
        size: attachment.size,
      })
    }
  } catch (error) {
    for (const ref of refs) if (ref.id) await release(ref.id).catch(() => {})
    if (isPisperError(error)) throw error
    throw pisperError('failed', 'attachment', 'could not store the attachment')
  }
  return refs
}

/**
 * 叫醒时交给席位会话的附件：按引用读回内容，拼成会话输入框发的那种形状。
 * 几句话并在一次叫醒里时合起来也守会话的上限（8 个、20 MB）；超出的、资产已经不在的只写名字。
 * @param {Record<string, any>[]} messages 触发这次叫醒的消息（按顺序）
 * @param {(assetId: string) => Promise<{ size: number, buffer?: Buffer } | null>} readAsset
 * @param {Map<string, Promise<{ size: number, text?: string, data?: string } | null>>} [shared]
 *   同一条消息叫醒好几个 Bot 时共用的读取结果（@所有人 带大附件时不各读一遍、各转一遍 base64）。
 */
export async function wakeAttachments(messages, readAsset, shared = new Map()) {
  // 读一次、转一次：文字按 UTF-8，其余按 base64；几个席位拿到的是同一个字符串。
  const load = (ref) => {
    let pending = shared.get(ref.id)
    if (!pending) {
      pending = readAsset(ref.id).then(
        (stored) => {
          const buffer = stored?.buffer
          if (!buffer?.length) return null
          return ref.kind === 'text'
            ? { size: buffer.length, text: buffer.toString('utf8') }
            : { size: buffer.length, data: buffer.toString('base64') }
        },
        () => null,
      )
      shared.set(ref.id, pending)
    }
    return pending
  }
  const attachments = []
  const skipped = []
  let total = 0
  for (const message of messages) {
    for (const ref of roomAttachmentRefs(message.attachments)) {
      if (attachments.length >= MAX_ROOM_ATTACHMENTS) {
        skipped.push(ref.name)
        continue
      }
      if (ref.kind === 'path') {
        attachments.push({ id: `path:${ref.path}`, kind: 'path', name: ref.name, path: ref.path })
        continue
      }
      const content = await load(ref)
      if (!content || total + content.size > MAX_ROOM_ATTACHMENTS_BYTES) {
        skipped.push(ref.name)
        continue
      }
      total += content.size
      const common = { id: ref.id, name: ref.name, mimeType: ref.mimeType, size: content.size }
      if (ref.kind === 'text') attachments.push({ ...common, kind: 'text', text: content.text })
      else if (ref.kind === 'image')
        attachments.push({ ...common, kind: 'image', data: content.data })
      else
        attachments.push({
          ...common,
          kind: 'document',
          extension: String(ref.name.split('.').at(-1) || '').toLowerCase(),
          data: content.data,
        })
    }
  }
  return { attachments, skipped }
}

// 这份资产是不是只被群聊消息用过：主记录和每条引用都是群里上传的、都不属于任何会话。
// 有一条来自会话（你在别处发过同一个文件，或 Bot 已经收到过它）就不删。
export function isRoomOnlyAsset(asset) {
  if (!asset) return false
  const entries = [asset, ...(Array.isArray(asset.references) ? asset.references : [])]
  return entries.every(
    (entry) => entry?.source === ROOM_ASSET_SOURCE && !String(entry?.sessionId || ''),
  )
}

// 还有哪条没撤回的群消息引用着这份资产。
export function roomsStillUse(messageLists, assetId) {
  return messageLists.some((messages) =>
    messages.some(
      (message) =>
        !message.recalled &&
        Array.isArray(message.attachments) &&
        message.attachments.some((ref) => ref?.id === assetId),
    ),
  )
}
