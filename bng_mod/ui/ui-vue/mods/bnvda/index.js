import { installBNVDA } from './bnvdaRuntime.js'
import { loadingScreen } from '@/services/screenCover.js'
import { watch } from 'vue'

let uninstallRuntime = null
let loadGeneration = 0

export async function onLoad() {
  const generation = ++loadGeneration
  console.info('[bnvda] Vue mod discovered; waiting for UI bootstrap.')

  try {
    const bootstrap = window.bngUiBootstrap
    if (!bootstrap || !bootstrap.whenDone) {
      throw new Error('window.bngUiBootstrap.whenDone is unavailable')
    }
    await bootstrap.whenDone
    if (generation !== loadGeneration) return

    console.info('[bnvda] UI bootstrap complete; installing runtime.')
    const vue = window.bngVue || {}
    const stores = vue.stores || {}
    const controls = vue.Controls || vue.controls || stores.Controls || stores.controls
    uninstallRuntime = installBNVDA(window.globalAngularRootScope, {
      controls,
      icons: vue.icons,
      loadingScreen,
      watch,
    })
  } catch (error) {
    if (generation === loadGeneration) console.error('[bnvda] Vue mod startup failed.', error)
  }
}

export async function onUnload() {
  ++loadGeneration
  if (typeof uninstallRuntime === 'function') uninstallRuntime()
  uninstallRuntime = null
  console.info('[bnvda] Vue mod unloaded.')
}