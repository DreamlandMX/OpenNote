export { ShellApp, type ShellAppProps, type VaultAction } from './ShellApp';
export { Sidebar, type Screen } from './Sidebar';
export { WelcomeScreen, type WelcomeScreenProps } from './WelcomeScreen';
export { HomeScreen, type HomeScreenProps } from './HomeScreen';
export { SettingsScreen, type SettingsScreenProps } from './SettingsScreen';
export { LocaleSelect, type LocaleSelectProps } from './LocaleSelect';
export {
  changeLocale,
  type LocaleChangePort,
  type LocaleChangeResult,
} from './locale-change';
export { WorkspaceScreen, type WorkspaceScreenProps, type WorkspaceGate, type GuardBlockedEntry } from './WorkspaceScreen';
export {
  disconnectedShellPort,
  createTauriShellPort,
  resolveShellPort,
  type ShellPort,
  type ShellPortKind,
  type ShellCallResult,
} from './port';
export {
  createTauriNoteService,
  resolveNoteService,
  newOperationId,
  type NoteService,
} from './tauri-note-service';
export {
  readDevicePreferences,
  writeDevicePreferences,
  pushRecent,
  recentCommandLimit,
  recentNoteLimit,
  searchHistoryLimit,
  clearSearchHistory,
  rememberSearchHistoryQuery,
  type DevicePreferences,
  type ThemePreference,
} from './device-prefs';
export { CommandPalette, QuickOpen } from './CommandSurface';
export {
  createCommandRegistry,
  orderByRecent,
  type CommandAvailability,
  type CommandDefinition,
  type CommandId,
  type CommandMatch,
  type CommandRegistry,
} from './commands';
export {
  breadcrumbOf,
  quickOpenTitle,
  rankQuickOpen,
  scoreQuickOpen,
  truncateQuery,
  MAX_QUICK_OPEN_QUERY,
  type QuickOpenCandidate,
  type QuickOpenMatch,
} from './quick-open';
export { OutlinePanel } from './OutlinePanel';
export {
  buildOutline,
  currentSection,
  lineStartOffsets,
  moveOutline,
  outlineIsStale,
  toggleCollapsed,
  visibleOutline,
  type OutlineRow,
} from './outline';
export { TagPanel } from './TagPanel';
export {
  filterTags,
  hasMissingNotes,
  MAX_TAG_LENGTH,
  MIN_TAG_LENGTH,
  normalizeTag,
  notesForTag,
  tagRows,
  tagSummary,
  validateTag,
  type TagRow,
  type TagValidation,
} from './tags';
export {
  closeTab,
  cycleTab,
  isFavorite,
  recentLabel,
  removeRecent,
  resolveFavorites,
  resolveRecent,
  tabNeedsGuard,
  toggleFavorite,
  type NoteReferenceEntry,
  type TabState,
} from './tabs';
export { SlashMenu, SLASH_LABEL_KEYS, slashLabels } from '../editor/SlashMenu';
export {
  codeInsertion,
  detectSlashQuery,
  equationInsertion,
  headingInsertion,
  linkInsertion,
  rankSlashItems,
  SLASH_ITEMS,
  tableInsertion,
  taskInsertion,
  textInsertion,
  validateTableSize,
  visibleSlashItems,
  wrapText,
  DEFAULT_TABLE_COLUMNS,
  DEFAULT_TABLE_ROWS,
  MAX_EQUATION_CHARS,
  TABLE_MAX_COLUMNS,
  TABLE_MAX_ROWS,
  type FragmentKind,
  type Insertion,
  type InsertionError,
  type InsertionResult,
  type Selection,
  type SlashItem,
} from '../editor/insert';
export { ExportPanel } from './ExportPanel';
export { ImportPanel } from './ImportPanel';
export {
  appearanceStyle,
  bodySizeLabel,
  clampBodySize,
  clampLineMeasure,
  defaultMeasureFor,
  lineMeasureLabel,
  MAX_BODY_SIZE,
  MAX_LINE_MEASURE,
  MIN_BODY_SIZE,
  MIN_LINE_MEASURE,
  resolveAppearance,
  DEFAULT_BODY_SIZE,
  DEFAULT_CJK_MEASURE,
  DEFAULT_LATIN_MEASURE,
  type ReadingAppearance,
} from './appearance';
export {
  applyBinding,
  bindChord,
  chordKey,
  chordText,
  DEFAULT_BINDINGS,
  effectiveBinding,
  MAX_CHORD_STEPS,
  parseChord,
  parseBindings,
  resetBinding,
  validateChord,
  chordFromEvent,
  matchesCommand,
  resolveCommand,
  type KeyEventLike,
  type ResolvedCommand,
  type BindableCommandId,
  type BindingResult,
  type Chord,
  type ChordError,
  type ChordStep,
  type Modifier,
} from './shortcuts';
export { ShortcutSettings, bindingsRoundTrip } from './ShortcutSettings';
export { BINDABLE_COMMANDS, bindableCommand, isBindableCommandId } from './command-catalogue';
export { resolveStep, type ChordPrefix, type StepResolution } from './shortcuts';
export {
  COMMON_TIME_ZONES,
  firstDayOfWeek,
  formatCalendarDate,
  formatCount,
  formatInstant,
  formatNumber,
  localCalendarDate,
  machineDate,
  validateTimeZone,
  zoneLabel,
  type DateStyle,
  type RegionalPreferences,
  type WeekInfo,
} from './regional';
export { TaskPanel } from './TaskPanel';
export {
  filterCounts,
  filterTasks,
  groupOf,
  groupTasks,
  GROUP_ORDER,
  inFolder,
  sourceFolders,
  toggleRequest,
  dueAttribute,
  type TaskFilter,
  type TaskGroup,
  type TaskGroupKey,
  type VisibleTask,
} from './task-view';
export { TemplatePanel } from './TemplatePanel';
export {
  insertable,
  insertAtSelection,
  needsAttention,
  noteNameFor,
  summarizeWarnings,
  templateBody,
  toChoices,
  variablesFor,
  type TemplateChoice,
  type WarningSummary,
} from './template-view';
export {
  actionFor,
  isCalendarDay,
  shiftDate,
  statusKey,
  weekdayIndex,
  WEEKDAY_KEYS,
  type DailyAction,
} from './daily-view';
export { GraphPanel } from './GraphPanel';
export {
  edgeLine,
  filterNodes,
  filterReport,
  layoutGraph,
  neighbours,
  NODE_RADIUS,
  GRAPH_RADIUS,
  shownDepth,
  summarizeGraph,
  type GraphLayout,
  type GraphSummary,
  type PositionedEdge,
  type PositionedNode,
} from './graph-view';
export { DatabasePanel } from './DatabasePanel';
export {
  ariaSort,
  cellText,
  describeFilters,
  isEmptyDueToFilters,
  isUnset,
  nextSort,
  operatorsFor,
  rowKey,
  tableSummary,
  toFilter,
  type FilterDraft,
} from './table-view';
export { CanvasPanel } from './CanvasPanel';
export {
  beginDrag,
  commitDrag,
  connectNodes,
  dragRect,
  exportSummary,
  isValidRect,
  listItems,
  newTextNode,
  removeNodeLocally,
  resizeRect,
  MIN_NODE_HEIGHT,
  MIN_NODE_WIDTH,
  type CanvasListItem,
  type DragState,
  type Rect,
} from './canvas-view';
export { DiagnosticsPanel } from './DiagnosticsPanel';
export {
  correlationText,
  declaresRequiredOmissions,
  displayFields,
  fieldText,
  includesLog,
  isHealthy,
  isUnknownValue,
  orderScopes,
  redactionSummary,
  REQUIRED_OMISSIONS,
  statusKey as diagnosticStatusKey,
  type DisplayField,
  type StatusKey,
} from './diagnostics-view';
export { ProjectWizardPanel } from './ProjectWizardPanel';
export {
  canCreate as canCreateProject,
  defaultDestination,
  defaultMetadata,
  initialLanguage,
  issueKey as scaffoldIssueKey,
  issueParams as scaffoldIssueParams,
  metadataIssue,
  paperOptions,
  planSummary as scaffoldPlanSummary,
  previewFiles as scaffoldPreviewFiles,
  type MetadataIssue,
} from './scaffold-view';
export { LatexToolbar } from './LatexToolbar';
export {
  applyResult as applyAssistResult,
  completionRange,
  describeResult as describeAssistResult,
  editsAreSafe,
  findCandidate,
  groupCandidates,
  isBlockedByComposition,
  isValidRange as isValidAssistRange,
  languageLabel,
  previewEdit,
  type EditPreview,
} from './latex-assist-view';
export { InsertPanel } from './InsertPanel';
export {
  bibliographySummary,
  canInsert,
  citationItems,
  citableItems,
  DEFAULT_IMAGE_WIDTH,
  fieldIssue as insertionFieldIssue,
  filterCitations,
  issuesFor as insertionIssuesFor,
  labelPrefix,
  missingDependencyLines,
  parseTableInput,
  suggestLabel,
  type CitationItem,
  type FieldIssue,
} from './insertion-form';
export { CitationPanel } from './CitationPanel';
export {
  bibliographyState,
  canInsertCitation,
  citationIssueKey,
  citationIssueParams,
  citationIssueText,
  isSelectable,
  orderedSelection,
  pruneSelection,
} from './citation-panel';
export { ProjectInspectPanel } from './ProjectInspectPanel';
export {
  fileCount,
  findingKey as projectFindingKey,
  findingParams as projectFindingParams,
  findingText as projectFindingText,
  hasAmbiguity,
  isBlocking as isBlockingFinding,
  parseFileListing,
  partitionFindings,
  rankedCandidates,
  unknownDependencies,
  verdictKey as projectVerdictKey,
  type FindingKey,
} from './project-inspect-view';
export { TagRenameDialog } from './TagRenameDialog';
export {
  actionKey as tagActionKey,
  canApply as canApplyTagPlan,
  changeDescription as tagChangeDescription,
  conflictRow,
  conflictRows,
  conflictText as tagConflictText,
  isChange as isTagChange,
  notesForPlan,
  planSummary as tagPlanSummary,
  rewriteRows,
  type ConflictRow,
  type RewriteRow,
} from './tag-rename-view';
export { RenameDialog } from './RenameDialog';
export {
  canCommit as canCommitRename,
  commitSummary,
  expectedRevisions,
  folderOf,
  issueRow as renameIssueRow,
  issueRows as renameIssueRows,
  issueText as renameIssueText,
  needsExtension,
  normalizeDestination,
  stepRows as renameStepRows,
  warningKey,
  warningText,
  withExtension,
  type IssueRow as RenameIssueRow,
  type StepRow as RenameStepRow,
} from './rename-view';
export { AttachmentPanel } from './AttachmentPanel';
export {
  formatBytes as formatAttachmentBytes,
  isBlocking as isBlockingAttachmentIssue,
  issueKey as attachmentIssueKey,
  issueParams as attachmentIssueParams,
  issueText as attachmentIssueText,
  kindKey as attachmentKindKey,
  looksLikeSha256,
  parseFileList,
  partitionPlan as partitionAttachmentPlan,
  planVerdict as attachmentPlanVerdict,
  previewKey as attachmentPreviewKey,
  willStore,
  type ParsedFile,
} from './attachment-view';
export { SplitControls } from './SplitControls';
export {
  activeNoteId as activePaneNoteId,
  applySelection as applyPaneSelection,
  closePane,
  dropNote as dropNoteFromPanes,
  dropSelection as dropPaneSelection,
  focusOtherPane,
  focusPane,
  initialLayout as initialPaneLayout,
  isNarrow,
  layoutTabs,
  MAX_PANES,
  NARROW_BREAKPOINT_PX,
  openInActivePane,
  renderedPanes,
  selectionOf as paneSelectionOf,
  sharesNote,
  splitPane,
  toggleOrientation,
  visibleColumns,
  type Orientation,
  type Pane,
  type PaneLayout,
  type PaneSelection,
} from './panes';
