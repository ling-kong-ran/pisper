import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import {
  createHashRouter,
  Link,
  Outlet,
  RouterProvider,
  useLocation,
  useNavigate,
} from 'react-router-dom'
import { I18nextProvider } from 'react-i18next'
import { UnsavedChangesProvider } from '@/app/UnsavedChangesProvider'
import { i18n } from '@/app/i18n'
import { ProviderWorkbench } from '@/features/config/ProviderWorkbench'
import type { ConfigData } from '@/features/config/config-types'
import '@/index.css'

const initial: ConfigData = {
  provider: 'a',
  model: 'model-a',
  thinkingLevel: 'medium',
  toolMode: 'full',
  providers: ['a', 'b'].map((id) => ({
    id,
    name: `Provider ${id.toUpperCase()}`,
    type: 'chat',
    api: 'openai-responses',
    baseUrl: `https://${id}.example.test/v1`,
    configured: true,
    enabled: true,
    models: [{ id: `model-${id}`, name: `Model ${id}`, kind: 'chat', reasoning: true }],
  })),
}
function Models() {
  const [config, setConfig] = useState(initial)
  const [selected, setSelected] = useState('a')
  return (
    <ProviderWorkbench
      config={config}
      selectedProviderId={selected}
      onSelectProvider={setSelected}
      toggling=""
      settingDefault=""
      settingModel=""
      deletingModel=""
      onSave={setConfig}
      onClone={() => {}}
      onDelete={() => {}}
      onDeleteModel={() => {}}
      onToggle={() => {}}
      onSetDefault={() => {}}
      onSetDefaultModel={() => {}}
    />
  )
}
function Shell() {
  const location = useLocation()
  const navigate = useNavigate()
  return (
    <main className="p-6">
      <nav className="flex gap-4">
        <Link to="/config/models">Models</Link>
        <Link to="/config/interface">Interface</Link>
        <Link to="/config/updates">Updates</Link>
        <button onClick={() => navigate('/config/interface', { replace: true })}>
          Replace interface
        </button>
      </nav>
      <output data-testid="location">{location.pathname + location.search}</output>
      <Outlet />
    </main>
  )
}
if (!location.hash) location.hash = '/config/models'
const router = createHashRouter([
  {
    element: (
      <UnsavedChangesProvider>
        <Shell />
      </UnsavedChangesProvider>
    ),
    children: [
      { path: '/config/models', element: <Models /> },
      { path: '/config/interface', element: <p>Interface destination</p> },
      { path: '/config/updates', element: <p>Updates destination</p> },
    ],
  },
])
await i18n.changeLanguage('en-US')
createRoot(document.getElementById('root')!).render(
  <I18nextProvider i18n={i18n}>
    <RouterProvider router={router} />
  </I18nextProvider>,
)
