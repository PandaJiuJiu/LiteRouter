import { beforeEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ElementPlus from 'element-plus'
import { createRouter, createMemoryHistory } from 'vue-router'
import i18n, { setLocale } from '../src/i18n'
import zhCN from '../src/i18n/locales/zh-CN'

const listChannels = vi.fn()
const testModel = vi.fn()
const updateChannelModels = vi.fn()
const fetchModels = vi.fn()
vi.mock('../src/api', () => ({ listChannels, testModel, updateChannelModels, fetchModels }))

const { default: Models } = await import('../src/views/Models.vue')

/** One channel serving a single model over the OpenAI-compatible URL. */
const CHANNEL = {
  id: 1,
  name: 'ark',
  base_url: 'https://ark.example',
  base_url_anthropic: '',
  models: 'gpt-4o',
  disabled_models: '',
  enabled: 1,
}

async function mountModels() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }],
  })
  const w = mount(Models, { global: { plugins: [ElementPlus, i18n, router] } })
  await flushPromises()
  return w
}

/** The per-protocol status lines on the card for `model`, as `{ ok, text }`. */
const statusLines = (w, model = 'gpt-4o') =>
  w.findAll('.model-card').find((c) => c.find('.model-name').text() === model)
    .findAll('.status-line')
    .map((l) => ({ ok: l.classes().includes('ok'), text: l.text() }))

beforeEach(() => {
  listChannels.mockReset()
  testModel.mockReset()
  updateChannelModels.mockReset()
  fetchModels.mockReset()
  setLocale('zh-CN')
  listChannels.mockResolvedValue([CHANNEL])
})

describe('model test result', () => {
  it('shows the round-trip latency instead of the word "Available"', async () => {
    // The word was redundant with the check icon; the latency is the part the
    // admin actually wanted — which of the two protocols is the slow one.
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 842 } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)).toEqual([{ ok: true, text: 'openai842 ms' }])
  })

  it('keeps availability in the icon, not the text, when the probe fails', async () => {
    testModel.mockResolvedValue({
      ok: false,
      protocols: { openai: { ok: false, ms: 120, error: 'HTTP 401: invalid api key' } },
    })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    const [line] = statusLines(w)
    expect(line.ok).toBe(false)
    expect(line.text).toContain('120 ms')
    expect(line.text).not.toContain(zhCN.models.available)
  })

  it('switches to seconds above a second so a timeout is readable at a glance', async () => {
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 10_040 } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)[0].text).toContain('10.04 s')
  })

  it('falls back to a dash when the backend sends no latency', async () => {
    // An older backend, or a transport error caught client-side, leaves `ms`
    // undefined — the card must still render rather than show "undefined ms".
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true } } })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w)[0].text).toContain('—')
  })

  it('reports both protocols side by side when both are tested', async () => {
    testModel.mockResolvedValue({
      ok: true,
      protocols: {
        openai: { ok: true, ms: 300 },
        anthropic: { ok: true, ms: 2_500 },
      },
    })
    const w = await mountModels()
    await w.find('.model-card .action-btn').trigger('click')
    await flushPromises()

    expect(statusLines(w).map((l) => l.text)).toEqual([
      'openai300 ms',
      'anthropic2.50 s',
    ])
  })
})

describe('test all', () => {
  // A model is in exactly one of the two lists — toggleModel deletes it from
  // the other — so the fixture must keep them disjoint or isSelected() reads
  // the enabled list and the test proves nothing.
  const channelWith = (models, disabled_models) => ({ ...CHANNEL, models, disabled_models })

  /** The model names each `testModel` call asked for, in call order. */
  const tested = () => testModel.mock.calls.map((c) => c[0].model)

  /** The channel-level "test all" button — not the toolbar's refresh. */
  const testAllBtn = (w) =>
    w.findAll('.el-button').find((b) => b.text().includes(zhCN.models.testAll))

  it('skips disabled models', async () => {
    // A disabled model is not routed to, so probing it tells the admin nothing
    // — and on a channel with many disabled models it stretches "test all" to
    // minutes of waiting for cards that are greyed out anyway.
    listChannels.mockResolvedValue([channelWith('gpt-4o', 'gpt-4o-mini')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    expect(tested()).toEqual(['gpt-4o'])
  })

  it('still leaves disabled cards untested rather than marked failed', async () => {
    // The skipped card must keep saying "Not tested": a red ✗ on a model the
    // admin deliberately switched off would read as a real fault.
    listChannels.mockResolvedValue([channelWith('gpt-4o', 'gpt-4o-mini')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    const skipped = w.findAll('.model-card')
      .find((c) => c.find('.model-name').text() === 'gpt-4o-mini')
    expect(skipped.find('.status-untested').text()).toBe(zhCN.models.untested)
  })

  it('tests every model when they are all enabled', async () => {
    listChannels.mockResolvedValue([channelWith('gpt-4o,gpt-4o-mini,gpt-5', '')])
    testModel.mockResolvedValue({ ok: true, protocols: { openai: { ok: true, ms: 100 } } })
    const w = await mountModels()

    await testAllBtn(w).trigger('click')
    await flushPromises()

    expect(tested()).toEqual(['gpt-4o', 'gpt-4o-mini', 'gpt-5'])
  })

  it('disables the button when every model is switched off', async () => {
    // Otherwise the button is clickable but does nothing, which reads as a bug.
    listChannels.mockResolvedValue([channelWith('', 'gpt-4o')])
    const w = await mountModels()

    expect(testAllBtn(w).classes()).toContain('is-disabled')
    await testAllBtn(w).trigger('click')
    await flushPromises()
    expect(testModel).not.toHaveBeenCalled()
  })
})

describe('toolbar filters', () => {
  // Three channels with disjoint model names: a search for "gpt" must surface
  // ark + azure but not mistral; "enabled only" must keep the two channels
  // that still route traffic (ark, azure) and hide the disabled-only channel
  // (gpt-channels-azure is half disabled so it stays; mistral-disabled-gram
  // has no enabled model so it gets hidden).
  // Wait, for `enabledOnly`, the rule is: the channel must have at least one
  // enabled model. ark=1 enabled, azure=1 enabled, mistral=1 disabled → only
  // ark and azure pass.
  const override = (overrides) => ({ ...CHANNEL, ...overrides })
  const CHANNELS = () => [
    override({ id: 1, name: 'ark', models: 'gpt-4o', disabled_models: '' }),
    override({ id: 2, name: 'azure', models: 'gpt-4o-mini', disabled_models: 'gpt-4' }),
    override({ id: 3, name: 'mistral', models: '', disabled_models: 'mistral-large' }),
  ]

  const visibleCards = (w) =>
    w.findAll('.channel-card').map((c) => c.find('.channel-name').text())

  beforeEach(() => {
    listChannels.mockResolvedValue(CHANNELS())
  })

  it('summary counts models across every channel regardless of filter', async () => {
    // 1 + 2 + 1 = 4 models (enabled + disabled, deduped), 3 channels,
    // 2 disabled — the chips reflect the *whole* DB, not the filtered view,
    // so they don't flicker while the admin types in the search box.
    const w = await mountModels()
    await flushPromises()

    const text = w.find('.summary').text()
    expect(text).toContain(zhCN.models.summaryModels.replace('{count}', '4'))
    expect(text).toContain(zhCN.models.summaryChannels.replace('{count}', '3'))
    expect(text).toContain(zhCN.models.summaryDisabled.replace('{count}', '2'))
  })

  it('search filters channels whose models match (case-insensitive)', async () => {
    // A "GPT" search must surface the two gpt-* channels but not mistral.
    // "gpt-4" lives in azure's disabled list, so that channel shows up too —
    // the filter is across both lists, not just the enabled set.
    const w = await mountModels()
    await flushPromises()

    await w.find('.filter-search input').setValue('GPT')
    await flushPromises()

    const visible = visibleCards(w)
    expect(visible).toContain('ark')
    expect(visible).toContain('azure')
    expect(visible).not.toContain('mistral')
  })

  it('enabledOnly hides channels that have no enabled model', async () => {
    // mistral's only model is disabled — the channel routes nothing, so
    // "enabled only" hides it. ark and azure both have an enabled entry,
    // so they stay. This is the inverse of the previous "disabled only"
    // toggle: pick the one that matches what you actually want to see.
    const w = await mountModels()
    await flushPromises()

    await w.find('.filter-toggle .el-switch').trigger('click')
    await flushPromises()

    const visible = visibleCards(w)
    expect(visible).toContain('ark')
    expect(visible).toContain('azure')
    expect(visible).not.toContain('mistral')
  })
})

describe('drag to reorder', () => {
  // Three cards laid out two-per-row, so the multi-column geometry is actually
  // exercised rather than collapsing to a single column of "everything is
  // before everything".
  const MULTI = {
    ...CHANNEL,
    models: 'gpt-4o,gpt-4o-mini',
    disabled_models: 'gpt-4',
  }
  const RECTS = {
    'gpt-4o': { left: 0, top: 0, width: 200, height: 80 },
    'gpt-4o-mini': { left: 220, top: 0, width: 200, height: 80 },
    'gpt-4': { left: 0, top: 92, width: 200, height: 80 },
  }

  const order = (w) => w.findAll('.model-card .model-name').map((n) => n.text())
  const card = (w, m) => w.find(`.model-card[data-model="${m}"]`)

  /** jsdom has neither DragEvent nor DataTransfer, so build the event by hand:
   *  MouseEvent for the ones that carry coordinates, plain Event for the rest. */
  function fireDrag(target, type, init = {}) {
    const el = target.element || target
    const Ev = type === 'dragover' || type === 'drop' ? window.MouseEvent : window.Event
    const ev = new Ev(type, { bubbles: true, cancelable: true, ...init })
    if (init.dataTransfer) Object.defineProperty(ev, 'dataTransfer', { value: init.dataTransfer })
    el.dispatchEvent(ev)
    return ev
  }

  const DATA_TRANSFER = { setData() {}, getData: () => '', effectAllowed: '', dropEffect: '' }

  /** jsdom reports every rect as 0x0, which would make the nearest-centre
   *  search degenerate to "always card 0". */
  function stubRects(map = RECTS) {
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function () {
      const r = map[this.dataset?.model] || { left: 0, top: 0, width: 0, height: 0 }
      return { ...r, right: r.left + r.width, bottom: r.top + r.height, x: r.left, y: r.top, toJSON: () => r }
    })
  }

  /** dragstart on `from`, then a drop at (x, y) over the grid. */
  async function dragOnto(w, from, x, y) {
    fireDrag(card(w, from), 'dragstart', { dataTransfer: DATA_TRANSFER })
    const grid = w.find('.model-grid')
    fireDrag(grid, 'dragover', { clientX: x, clientY: y, dataTransfer: DATA_TRANSFER })
    await flushPromises()
    fireDrag(grid, 'drop', { clientX: x, clientY: y, dataTransfer: DATA_TRANSFER })
    await flushPromises()
  }

  beforeEach(() => {
    // A fresh copy every time: persistOrder writes ch.models / ch.disabled_models
    // back onto the object listChannels handed out, and mockResolvedValue
    // returns the same reference — so a shared fixture leaks one test's reorder
    // into every test that follows.
    listChannels.mockImplementation(() => Promise.resolve([{ ...MULTI }]))
    updateChannelModels.mockResolvedValue({})
    stubRects()
  })

  it('persists the reordered union as the new CSV, in the new order', async () => {
    const w = await mountModels()

    // drop on the right half of gpt-4o-mini → insert after it
    await dragOnto(w, 'gpt-4o', 380, 40)

    expect(updateChannelModels).toHaveBeenCalledTimes(1)
    expect(updateChannelModels).toHaveBeenCalledWith(1, 'gpt-4o-mini,gpt-4o', 'gpt-4')
    expect(order(w)).toEqual(['gpt-4o-mini', 'gpt-4o', 'gpt-4'])
  })

  it('inserts before a card when the pointer is in its left half', async () => {
    const w = await mountModels()

    // Pointer in the left half of gpt-4o (spans 0..200, centre 100)
    // and inside its row band, so `after` flips on the X axis. gpt-4 at
    // (100,132) is the other candidate but is further away in 2D, so it
    // doesn't win the nearest-centre search.
    await dragOnto(w, 'gpt-4o-mini', 10, 40)

    expect(updateChannelModels).toHaveBeenCalledWith(1, 'gpt-4o-mini,gpt-4o', 'gpt-4')
  })

  it('uses the vertical axis when the pointer is outside the target row band', async () => {
    const w = await mountModels()

    // y=180 is below gpt-4's band (92..172), so the decision falls to the Y
    // axis: 180 is past gpt-4's midpoint, so the card goes after it — the
    // end of the list. Nearest by centre is gpt-4 (100,132).
    await dragOnto(w, 'gpt-4o', 100, 180)

    expect(updateChannelModels).toHaveBeenCalledWith(1, 'gpt-4o-mini,gpt-4o', 'gpt-4')
    expect(order(w)).toEqual(['gpt-4o-mini', 'gpt-4', 'gpt-4o'])
  })

  it('keeps a disabled model’s move inside disabled_models only', async () => {
    // Two disabled models, so a move actually changes the stored
    // `disabled_models` string. With only one disabled model that string can
    // never change and the partition would assert nothing.
    listChannels.mockImplementation(() =>
      Promise.resolve([{ ...MULTI, models: 'gpt-4o,gpt-4o-mini', disabled_models: 'gpt-4,claude-3' }]))
    const w = await mountModels()

    // claude-3 (index 3) → the front slot, before gpt-4o.
    await dragOnto(w, 'claude-3', 10, 40)

    // The enabled CSV is byte-identical; only the disabled one reordered.
    expect(updateChannelModels).toHaveBeenCalledWith(1, 'gpt-4o,gpt-4o-mini', 'claude-3,gpt-4')
    expect(order(w)).toEqual(['claude-3', 'gpt-4o', 'gpt-4o-mini', 'gpt-4'])
  })

  it('rolls the card order back when the save fails', async () => {
    updateChannelModels.mockRejectedValue({ response: { data: { message: 'boom' } } })
    const w = await mountModels()

    await dragOnto(w, 'gpt-4o', 380, 40)

    // The component's ch.models is internal — DOM order is the only
    // observable surface, which is exactly why the rollback has to restore
    // `known` and not just the two CSV strings.
    expect(order(w)).toEqual(['gpt-4o', 'gpt-4o-mini', 'gpt-4'])
  })

  it('spends no request when the card lands back in its own slot', async () => {
    const w = await mountModels()

    // Drag the last card into the empty gutter to the right of the grid. Its
    // own slot is excluded as a candidate, so the nearest card is
    // gpt-4o-mini and "after it" resolves to the insertion index it already
    // occupies — applyMove is the identity, so nothing is written.
    await dragOnto(w, 'gpt-4', 500, 40)

    expect(updateChannelModels).not.toHaveBeenCalled()
    expect(order(w)).toEqual(['gpt-4o', 'gpt-4o-mini', 'gpt-4'])
  })

  it('refuses a second drag while the reorder save is in flight', async () => {
    updateChannelModels.mockReturnValue(new Promise(() => {}))
    const w = await mountModels()

    fireDrag(card(w, 'gpt-4o'), 'dragstart', { dataTransfer: DATA_TRANSFER })
    const grid = w.find('.model-grid')
    fireDrag(grid, 'dragover', { clientX: 380, clientY: 40, dataTransfer: DATA_TRANSFER })
    await flushPromises()
    fireDrag(grid, 'drop', { clientX: 380, clientY: 40, dataTransfer: DATA_TRANSFER })
    await flushPromises()

    expect(card(w, 'gpt-4o-mini').attributes('draggable')).toBe('false')
  })

  it('keeps the admin order through a later enable/disable toggle', async () => {
    // Three enabled models, so reordering produces an order that a Set-based
    // rebuild would visibly destroy if it didn't preserve insertion order.
    listChannels.mockImplementation(() =>
      Promise.resolve([{ ...MULTI, models: 'gpt-4o,gpt-4o-mini,o3', disabled_models: '' }]))
    const w = await mountModels()

    await dragOnto(w, 'o3', 380, 40) // o3 lands right after gpt-4o-mini
    updateChannelModels.mockClear()

    // toggleModel rebuilds ch.models from a Set — Set preserves insertion
    // order, so the curated order has to survive that.
    await card(w, 'gpt-4o-mini').find('.el-switch').trigger('click')
    await flushPromises()

    expect(updateChannelModels).toHaveBeenCalledWith(1, 'gpt-4o,o3', 'gpt-4o-mini')
  })

  it('sets the drag payload for browsers that need it', async () => {
    const dt = { setData: vi.fn(), getData: vi.fn(), effectAllowed: '', dropEffect: '' }
    const w = await mountModels()

    fireDrag(card(w, 'gpt-4o'), 'dragstart', { dataTransfer: dt })

    expect(dt.setData).toHaveBeenCalledWith('text/plain', 'gpt-4o')
    expect(dt.effectAllowed).toBe('move')
  })

  it('clears the drag marker on dragend', async () => {
    const w = await mountModels()
    fireDrag(card(w, 'gpt-4o'), 'dragstart', { dataTransfer: DATA_TRANSFER })
    fireDrag(w.find('.model-grid'), 'dragover', { clientX: 380, clientY: 40, dataTransfer: DATA_TRANSFER })
    await flushPromises()
    expect(w.findAll('.drop-before, .drop-after').length).toBeGreaterThan(0)

    fireDrag(card(w, 'gpt-4o'), 'dragend')
    await flushPromises()

    expect(w.findAll('.drop-before, .drop-after, .is-dragging')).toHaveLength(0)
  })
})
