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

    // The radial menu's centre is driven by RadialSVG: each wedge's focus and
    // blur call the centerFocus/centerBlur callbacks Radial.vue hands it, with
    // the item itself. (0.40 removed radialCenterCanvas.js; the centre is now a
    // Vue-rendered div fed from a private ref.) /src/modules is not in the base
    // bundle, so this comes through the runtime module graph -- keyed by
    // resolved id, so we get the very same class object Radial.vue imports and
    // can wrap its prototype. Dynamic and guarded: a resolve failure must cost
    // the radial menu, not the whole screen reader.
    let radialSvg = null
    try {
      const radialModule = await import('@/modules/radial/radialsvg')
      radialSvg = radialModule?.default || null
    } catch (error) {
      console.warn('[bnvda] RadialSVG unavailable; radial item names will not be spoken.', error)
    }
    if (generation !== loadGeneration) return

    console.info('[bnvda] UI bootstrap complete; installing runtime.')
    const vue = window.bngVue || {}
    const stores = vue.stores || {}
    // controlsStore FIRST: window.bngVue.controls is a hand-written facade
    // (main.js exposeControlsStore) that forwards a chosen subset of methods and
    // carries none of the store's reactive properties -- lastDevices among them,
    // which is how the runtime knows which pad the player is actually holding.
    // controlsStore is the pinia store itself and is a superset of the facade.
    const controls = vue.controlsStore || vue.Controls || vue.controls || stores.Controls || stores.controls
    uninstallRuntime = installBNVDA(window.globalAngularRootScope, {
      controls,
      icons: vue.icons,
      loadingScreen,
      radialSvg,
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