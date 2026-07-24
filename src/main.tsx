import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { RouterProvider } from 'react-router-dom'
import 'dockview-react/dist/styles/dockview.css'
import './index.css'
import { AppProviders } from './app/providers'
import { router } from './app/router'
import { legacyHashPath } from './app/routes'

const legacyPath = legacyHashPath(window.location.hash)
if (legacyPath) {
  window.history.replaceState(
    null,
    '',
    `${window.location.pathname}${window.location.search}#${legacyPath}`,
  )
}

const rootElement = document.getElementById('root')
if (!rootElement) throw new Error('Missing #root application mount point')

createRoot(rootElement).render(
  <StrictMode>
    <AppProviders>
      <RouterProvider router={router} />
    </AppProviders>
  </StrictMode>,
)
