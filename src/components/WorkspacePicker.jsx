import { useCallback, useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertTriangle, Check, ChevronRight, FolderOpen, RefreshCw, X } from 'lucide-react'
import { useI18n } from '../app/use-i18n.js'
import { apiJson } from '../lib/api.js'

export function WorkspacePicker({ session, onClose, onSelect }) {
  const { t } = useI18n()
  const [path, setPath] = useState(session.cwd || '')
  const [listing, setListing] = useState(null)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  const browse = useCallback(async (target) => {
    setLoading(true)
    setError('')
    try {
      const data = await apiJson(`/api/directories?path=${encodeURIComponent(target || '')}`)
      setPath(data.path)
      setListing(data)
    } catch (caught) {
      setError(caught.message)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    browse(session.cwd || '')
  }, [browse, session.cwd])

  const choose = async () => {
    setSaving(true)
    setError('')
    try {
      await onSelect(path)
    } catch (caught) {
      setError(caught.message)
    } finally {
      setSaving(false)
    }
  }

  return createPortal(
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <section className="modal workspace-modal" role="dialog" aria-modal="true" aria-label={t('设置工作目录')}>
        <div className="card-head"><div><h2>{t('设置工作目录')}</h2><p>{t('{name} 的工具和 Agent 将在此目录运行', { name: session.name })}</p></div><button className="icon-button" aria-label={t('关闭对话框')} onClick={onClose}><X size={17} /></button></div>
        <form className="workspace-path-form" onSubmit={(event) => { event.preventDefault(); browse(path) }}>
          <FolderOpen size={15} />
          <input value={path} onChange={(event) => setPath(event.target.value)} placeholder={t('输入项目的绝对路径')} autoFocus />
          <button className="button secondary" disabled={loading}>{loading ? <RefreshCw className="spin" size={13} /> : t('转到')}</button>
        </form>
        <div className="directory-browser">
          {listing?.parent && <button type="button" onClick={() => browse(listing.parent)}><FolderOpen size={14} /><span>..</span><small>{t('上级目录')}</small></button>}
          {listing?.directories.map((directory) => <button type="button" key={directory.path} onClick={() => browse(directory.path)}><FolderOpen size={14} /><span>{directory.name}</span><ChevronRight size={13} /></button>)}
          {!loading && listing && !listing.directories.length && <div className="directory-empty">{t('此目录没有子文件夹')}</div>}
          {loading && <div className="directory-empty"><RefreshCw className="spin" size={16} />{t('正在读取目录…')}</div>}
        </div>
        {error && <div className="config-error"><AlertTriangle size={13} />{error}</div>}
        <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose}>{t('取消')}</button><button type="button" className="button primary" onClick={choose} disabled={saving || loading || !path.trim()}>{saving ? <RefreshCw className="spin" size={14} /> : <Check size={14} />}{t(saving ? '切换中…' : '选择此目录')}</button></div>
      </section>
    </div>,
    document.body,
  )
}
