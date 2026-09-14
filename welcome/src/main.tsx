import React from 'react'
import { createRoot, hydrateRoot } from 'react-dom/client'
import App from './App'
import './styles/globals.css'

const container = document.getElementById('root')!
const app = (
  <React.StrictMode>
    <App />
  </React.StrictMode>
)

// `vite dev` serves the unprerendered shell, so the container is empty there.
if (container.firstChild) {
  hydrateRoot(container, app)
} else {
  createRoot(container).render(app)
}
