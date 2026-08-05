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

    // The radial menu paints its centre label onto a <canvas>, so the item name
    // never exists as DOM text. RadialCenterCanvas.setState is the last place it
    // is still a string. /src/modules is not in the base bundle, so this comes
    // through the runtime module graph -- keyed by resolved id, so we get the
    // very same class object Radial.vue imports and can wrap its prototype.
    // Dynamic and guarded: a resolve failure must cost the radial menu, not the
    // whole screen reader.
    let radialCenterCanvas = null
    try {
      const radialModule = await import('@/modules/radial/radialCenterCanvas')
      radialCenterCanvas = radialModule?.default || null
    } catch (error) {
      console.warn('[bnvda] RadialCenterCanvas unavailable; radial item names will not be spoken.', error)
    }
    if (generation !== loadGeneration) return

    console.info('[bnvda] UI bootstrap complete; installing runtime.')
    const vue = window.bngVue || {}
    const stores = vue.stores || {}
    const controls = vue.Controls || vue.controls || stores.Controls || stores.controls
    uninstallRuntime = installBNVDA(window.globalAngularRootScope, {
      controls,
      icons: vue.icons,
      loadingScreen,
      radialCenterCanvas,
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