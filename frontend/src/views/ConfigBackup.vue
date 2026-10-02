<template>
  <!-- Encrypted config backup. Admin-only — the file holds api_keys / sk-…
       tokens in plaintext inside the ciphertext, so handing it to a
       non-admin would defeat the point of the passphrase. `users` is
       deliberately not exportable.

       The component renders two dialogs and is otherwise empty; the
       settings row above just toggles `exportVisible` / `importVisible`.
       Each dialog resets its own state on `@closed` so reopening starts
       from a clean slate. -->
  <el-dialog
    v-model="exportVisible"
    :title="t('settings.backup.export.title')"
    width="540px"
    @closed="resetExport"
  >
    <p class="hint">{{ t('settings.backup.export.desc') }}</p>

    <el-checkbox-group v-model="sections" class="backup-sects">
      <el-checkbox value="channels">{{ t('settings.backup.section.channels') }}</el-checkbox>
      <el-checkbox value="tokens">{{ t('settings.backup.section.tokens') }}</el-checkbox>
      <el-checkbox value="mappings">{{ t('settings.backup.section.mappings') }}</el-checkbox>
    </el-checkbox-group>

    <div class="row">
      <el-input
        v-model="exportPass"
        type="password"
        show-password
        :placeholder="t('settings.backup.export.passPlaceholder')"
        class="row-input"
      />
      <el-input
        v-model="exportPassConfirm"
        type="password"
        show-password
        :placeholder="t('settings.backup.export.passConfirmPlaceholder')"
        class="row-input"
      />
    </div>
    <p v-if="exportError" class="err">{{ exportError }}</p>

    <template #footer>
      <el-button :disabled="exporting" @click="exportVisible = false">
        {{ t('common.cancel') }}
      </el-button>
      <el-button
        type="primary"
        :loading="exporting"
        :disabled="!exportValid || exporting"
        @click="doExport"
      >
        {{ t('settings.backup.export.button') }}
      </el-button>
    </template>
  </el-dialog>

  <el-dialog
    v-model="importVisible"
    :title="t('settings.backup.import.title')"
    width="640px"
    @closed="resetImport"
  >
    <p class="hint">{{ t('settings.backup.import.desc') }}</p>
    <div class="row">
      <el-upload
        :auto-upload="false"
        :show-file-list="false"
        :on-change="onFile"
        accept=".lrbak"
      >
        <el-button>{{ t('settings.backup.import.pickFile') }}</el-button>
      </el-upload>
      <span class="filename" v-if="importFileName">{{ importFileName }}</span>
      <el-input
        v-model="importPass"
        type="password"
        show-password
        :placeholder="t('settings.backup.import.passPlaceholder')"
        class="row-input"
      />
      <el-button
        :loading="previewing"
        :disabled="!importBytes || previewing"
        @click="doPreview"
      >
        {{ t('settings.backup.import.previewButton') }}
      </el-button>
    </div>
    <p v-if="importError" class="err">{{ importError }}</p>

    <template v-if="plan">
      <div class="plan-summary">
        <span class="badge ok">
          {{ t('settings.backup.import.summary.create', { n: plan.createCount }) }}
        </span>
        <span class="badge warn">
          {{ t('settings.backup.import.summary.update', { n: plan.updateCount }) }}
        </span>
        <span class="badge muted">
          {{ t('settings.backup.import.summary.skip', { n: plan.skipCount }) }}
        </span>
        <span class="badge info">
          {{ t('settings.backup.import.summary.rename', { n: plan.renameCount }) }}
        </span>
      </div>

      <div v-if="conflicts.length" class="conflict-list">
        <div v-for="c in conflicts" :key="c.key" class="conflict-row">
          <div class="conflict-meta">
            <span class="conflict-kind">{{ sectionLabel(c.kind) }}</span>
            <span class="conflict-name">{{ c.name }}</span>
          </div>
          <el-radio-group v-model="decisions[c.key]" size="small">
            <el-radio-button value="overwrite">
              {{ t('settings.backup.import.action.overwrite') }}
            </el-radio-button>
            <el-radio-button value="skip">
              {{ t('settings.backup.import.action.skip') }}
            </el-radio-button>
            <el-radio-button value="keep_both">
              {{ t('settings.backup.import.action.keepBoth') }}
            </el-radio-button>
          </el-radio-group>
          <span class="rename-hint" v-if="decisions[c.key] === 'keep_both'">
            {{ t('settings.backup.import.action.renameTo', { name: c.renamedTo }) }}
          </span>
        </div>
      </div>
    </template>

    <template v-else-if="committed">
      <div class="committed">
        <span class="badge ok">
          {{ t('settings.backup.import.committed.create', { n: committed.created }) }}
        </span>
        <span class="badge warn">
          {{ t('settings.backup.import.committed.update', { n: committed.updated }) }}
        </span>
        <span class="badge muted">
          {{ t('settings.backup.import.committed.skip', { n: committed.skipped }) }}
        </span>
        <span class="badge info">
          {{ t('settings.backup.import.committed.keptBoth', { n: committed.kept_both }) }}
        </span>
      </div>
    </template>

    <!-- One footer slot; its contents swap on plan/committed state.
         Keeping it as a single sibling (rather than wrapping each branch
         in its own <template #footer>) sidesteps a Vue codegen bug with
         conditional slots inside el-dialog. -->
    <template #footer>
      <template v-if="plan">
        <el-button :disabled="committing" @click="clearPlan">
          {{ t('settings.backup.import.cancel') }}
        </el-button>
        <el-button
          type="primary"
          :loading="committing"
          :disabled="!allConflictsDecided || committing"
          @click="doCommit"
        >
          {{ t('settings.backup.import.commitButton') }}
        </el-button>
      </template>
      <el-button v-else type="primary" @click="importVisible = false">
        {{ t('common.cancel') }}
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup>
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { commitImport, exportConfig, previewImport } from '../api'

const { t } = useI18n()

// The settings row above toggles these two dialogs. v-model wiring lives
// in the parent; the local mirror exists so the template can mutate it
// (close button, footer actions) without poking the prop.
const props = defineProps({
  exportVisible: { type: Boolean, default: false },
  importVisible: { type: Boolean, default: false },
})
const emit = defineEmits(['update:exportVisible', 'update:importVisible'])
const exportVisible = computed({
  get: () => props.exportVisible,
  set: (v) => emit('update:exportVisible', v),
})
const importVisible = computed({
  get: () => props.importVisible,
  set: (v) => emit('update:importVisible', v),
})

// What the user chose to back up. The backend enforces a non-empty list
// and only recognises these three spellings; the checkbox values match
// exactly so the SPA can hand the array off verbatim.
const DEFAULT_SECTIONS = ['channels', 'tokens', 'mappings']
const sections = ref([...DEFAULT_SECTIONS])
const sectionsValid = computed(() => sections.value.length > 0)

// ===== Export =====
const exportPass = ref('')
const exportPassConfirm = ref('')
const exporting = ref(false)
const exportError = ref('')
// True only when the user typed a non-empty passphrase twice AND they're
// identical AND at least 8 characters. The button reflects this so the
// click-time validation has a UI twin.
const exportValid = computed(
  () =>
    sectionsValid.value &&
    exportPass.value.length >= 8 &&
    exportPass.value === exportPassConfirm.value
)

function resetExport() {
  exportPass.value = ''
  exportPassConfirm.value = ''
  exportError.value = ''
  sections.value = [...DEFAULT_SECTIONS]
}

async function doExport() {
  exportError.value = ''
  if (exportPass.value !== exportPassConfirm.value) {
    exportError.value = t('settings.backup.export.passMismatch')
    return
  }
  if (exportPass.value.length < 8) {
    exportError.value = t('settings.backup.export.passTooShort')
    return
  }
  exporting.value = true
  try {
    const blob = await exportConfig(sections.value, exportPass.value)
    const url = URL.createObjectURL(blob.blob)
    const a = document.createElement('a')
    a.href = url
    a.download = blob.name
    a.click()
    URL.revokeObjectURL(url)
  } catch (e) {
    exportError.value = e?.response?.data?.error || String(e)
  } finally {
    exporting.value = false
  }
}

// ===== Import: pick file + passphrase, then preview, then commit =====
const importBytes = ref(null)
const importFileName = ref('')
const importPass = ref('')
const importError = ref('')
const previewing = ref(false)
const committing = ref(false)
const plan = ref(null)
const committed = ref(null)
const decisions = reactive({})

const allConflictsDecided = computed(() => {
  if (!plan.value) return false
  return plan.value.conflicts.every((c) => !!decisions[c.key])
})

const conflicts = computed(() => plan.value?.conflicts || [])

const sectionLabel = (kind) =>
  ({
    channel: t('settings.backup.section.channels'),
    token: t('settings.backup.section.tokens'),
    mapping: t('settings.backup.section.mappings'),
  })[kind] || kind

function resetImport() {
  importBytes.value = null
  importFileName.value = ''
  importPass.value = ''
  importError.value = ''
  clearPlan()
}

function onFile(file) {
  if (!file?.raw) return
  importFileName.value = file.name
  const reader = new FileReader()
  reader.onload = () => {
    importBytes.value = arrayBufferToBase64(reader.result)
  }
  reader.readAsArrayBuffer(file.raw)
}

// Encode the picked file as base64 — the SPA puts it in a JSON field and
// the backend decodes the same way, so the file's binary content survives
// the trip without a multipart detour.
function arrayBufferToBase64(buf) {
  let binary = ''
  const bytes = new Uint8Array(buf)
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + chunk))
  }
  return window.btoa(binary)
}

async function doPreview() {
  importError.value = ''
  previewing.value = true
  plan.value = null
  committed.value = null
  Object.keys(decisions).forEach((k) => delete decisions[k])
  try {
    const result = await previewImport({
      file: importBytes.value,
      passphrase: importPass.value,
    })
    plan.value = decorate(result)
  } catch (e) {
    importError.value = e?.response?.data?.error || String(e)
  } finally {
    previewing.value = false
  }
}

// Walk the server's plan and assemble the counts + a flat conflicts list
// the template can render. Renaming gets a suggested `<name>_1`; the
// server will find a higher suffix itself if `_1` is taken, so this is a
// preview, not a contract.
function decorate(result) {
  const sections = result.plan || {}
  const planConflicts = []
  let createCount = 0
  let updateCount = 0
  let skipCount = 0
  let renameCount = 0
  for (const [sectionName, list] of Object.entries(sections)) {
    for (const r of list) {
      if (!r.conflict) {
        createCount += 1
        continue
      }
      const key = `${sectionName}:${r.name}`
      planConflicts.push({ key, kind: r.kind, name: r.name, renamedTo: `${r.name}_1` })
      decisions[key] = 'overwrite'
    }
  }
  return { conflicts: planConflicts, createCount, updateCount, skipCount, renameCount }
}

async function doCommit() {
  if (!plan.value) return
  committing.value = true
  importError.value = ''
  try {
    const decisionsBySection = { channels: [], tokens: [], mappings: [] }
    for (const c of plan.value.conflicts) {
      const action = decisions[c.key]
      const [sectionName, ...rest] = c.key.split(':')
      decisionsBySection[sectionName].push({ name: rest.join(':'), action })
    }
    committed.value = await commitImport({
      file: importBytes.value,
      passphrase: importPass.value,
      decisions: decisionsBySection,
    })
    plan.value = null
  } catch (e) {
    importError.value = e?.response?.data?.error || String(e)
  } finally {
    committing.value = false
  }
}

function clearPlan() {
  plan.value = null
  committed.value = null
  Object.keys(decisions).forEach((k) => delete decisions[k])
}

// Exposed for tests. The template doesn't read these directly — it goes
// through `v-model` / `@click` — but jsdom doesn't simulate a real file
// picker or el-input's internal input event plumbing, so the spec drives
// state and handlers through this small API.
defineExpose({
  setExport({ pass, confirm }) {
    exportPass.value = pass
    exportPassConfirm.value = confirm
  },
  setImport({ bytes, name, pass }) {
    importBytes.value = bytes
    importFileName.value = name
    importPass.value = pass
  },
  // jsdom doesn't run the el-radio-button's change handler off a real DOM
  // click — Element Plus wires a hidden input that jsdom never receives —
  // so tests poke the reactive map directly. Production code never calls
  // this; it's a back door for the spec.
  setDecision(key, value) {
    decisions[key] = value
  },
  doExport,
  doPreview,
  doCommit,
})
</script>

<style scoped>
.backup-sects {
  margin-bottom: 16px;
}
.row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 8px 0;
  flex-wrap: wrap;
}
.row-input {
  width: 220px;
}
.filename {
  font-size: 12px;
  color: #606266;
}
.hint {
  font-size: 12px;
  color: #909399;
  margin: 0 0 8px;
}
.err {
  color: #f56c6c;
  font-size: 12px;
  margin: 6px 0 0;
}
.plan-summary,
.committed {
  display: flex;
  gap: 6px;
  margin: 12px 0;
  flex-wrap: wrap;
}
.conflict-list {
  margin: 12px 0;
}
.conflict-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 8px 0;
  border-top: 1px dashed #ebeef5;
  flex-wrap: wrap;
}
.conflict-meta {
  display: flex;
  gap: 8px;
  align-items: baseline;
  min-width: 280px;
}
.conflict-kind {
  font-size: 11px;
  color: #909399;
}
.conflict-name {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  color: #303133;
}
.rename-hint {
  font-size: 12px;
  color: #909399;
}
.badge {
  font-size: 12px;
  padding: 2px 8px;
  border-radius: 10px;
  border: 1px solid;
}
.badge.ok {
  color: #67c23a;
  border-color: #67c23a;
}
.badge.warn {
  color: #e6a23c;
  border-color: #e6a23c;
}
.badge.muted {
  color: #909399;
  border-color: #c0c4cc;
}
.badge.info {
  color: #409eff;
  border-color: #409eff;
}
</style>