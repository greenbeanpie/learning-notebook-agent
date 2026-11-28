package com.localnotebook.agent.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.localnotebook.agent.core.AppViewModel
import com.localnotebook.agent.core.L10n

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainScreen(vm: AppViewModel) {
    val state by vm.ui.collectAsState()
    val en = state.prefs.isEnglish
    var selectedTab by remember { mutableIntStateOf(1) } // 0: Sources, 1: Chat, 2: Studio
    var showNbMenu by remember { mutableStateOf(false) }

    val screenWidthDp = LocalConfiguration.current.screenWidthDp
    val isTablet = screenWidthDp >= 840

    if (state.showCreateNotebook) {
        CreateNotebookDialog(
            en = en,
            onDismiss = vm::dismissCreateNotebook,
            onCreate = vm::confirmCreateNotebook,
        )
    }

    Scaffold(
        topBar = {
            if (!state.studioFullscreen) {
                TopAppBar(
                    title = {
                        Box {
                            val activeTitle = state.notebooks.firstOrNull { it.id == state.activeNotebookId }?.title ?: ""
                            val focusManager = LocalFocusManager.current
                            var draft by remember(activeTitle) { mutableStateOf(activeTitle) }
                            var focused by remember { mutableStateOf(false) }
                            val hasNotebook = state.activeNotebookId.isNotEmpty()
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                BasicTextField(
                                    value = draft,
                                    onValueChange = { draft = it },
                                    enabled = hasNotebook,
                                    singleLine = true,
                                    textStyle = MaterialTheme.typography.titleMedium.copy(color = MaterialTheme.colorScheme.onSurface),
                                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                                    keyboardActions = KeyboardActions(onDone = { focusManager.clearFocus() }),
                                    modifier = Modifier
                                        .widthIn(min = 80.dp, max = 220.dp)
                                        .onFocusChanged { f ->
                                            if (focused && !f.isFocused) vm.renameNotebook(draft)
                                            focused = f.isFocused
                                        },
                                    decorationBox = { inner ->
                                        Box(contentAlignment = Alignment.CenterStart) {
                                            if (draft.isEmpty()) {
                                                Text(
                                                    L10n.t("nb.none_hint", en),
                                                    style = MaterialTheme.typography.titleMedium,
                                                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                                                )
                                            }
                                            inner()
                                        }
                                    },
                                )
                                IconButton(onClick = { showNbMenu = true }) {
                                    Icon(Icons.Default.ArrowDropDown, contentDescription = null)
                                }
                            }
                            DropdownMenu(expanded = showNbMenu, onDismissRequest = { showNbMenu = false }) {
                                state.notebooks.forEach { nb ->
                                    DropdownMenuItem(
                                        text = { Text(nb.title) },
                                        onClick = { showNbMenu = false; vm.selectNotebook(nb.id) },
                                    )
                                }
                                HorizontalDivider()
                                DropdownMenuItem(
                                    text = { Text("+ ${L10n.t("nb.new", en)}") },
                                    onClick = { showNbMenu = false; vm.createNotebook() },
                                )
                            }
                        }
                    },
                    actions = {
                        IconButton(onClick = { vm.setShowSettings(true) }) {
                            Icon(Icons.Default.Settings, contentDescription = L10n.t("settings.title", en))
                        }
                    },
                )
            }
        },
        bottomBar = {
            if (!isTablet && !state.studioFullscreen) {
                NavigationBar {
                    NavigationBarItem(
                        selected = selectedTab == 0,
                        onClick = { selectedTab = 0 },
                        icon = { Icon(Icons.Default.Folder, contentDescription = null) },
                        label = { Text(L10n.t("tab.sources", en)) },
                    )
                    NavigationBarItem(
                        selected = selectedTab == 1,
                        onClick = { selectedTab = 1 },
                        icon = { Icon(Icons.Default.ChatBubbleOutline, contentDescription = null) },
                        label = { Text(L10n.t("tab.chat", en)) },
                    )
                    NavigationBarItem(
                        selected = selectedTab == 2,
                        onClick = { selectedTab = 2 },
                        icon = { Icon(Icons.Default.AutoAwesome, contentDescription = null) },
                        label = { Text(L10n.t("tab.studio", en)) },
                    )
                }
            }
        },
    ) { padding ->
        Box(modifier = Modifier.fillMaxSize().padding(padding)) {
            Column(modifier = Modifier.fillMaxSize()) {
                if (state.statusBanner.isNotEmpty()) {
                    Surface(
                        color = MaterialTheme.colorScheme.surfaceVariant,
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                    ) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
                        ) {
                            Text(state.statusBanner, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
                            TextButton(onClick = { vm.dismissBanner() }) { Text(L10n.t("common.close", en)) }
                        }
                    }
                }

                if (isTablet && !state.studioFullscreen) {
                    Row(modifier = Modifier.weight(1f).fillMaxWidth()) {
                        SourcesScreen(
                            state = state,
                            onImportFile = vm::importFileUri,
                            onAddUrl = vm::addUrlSource,
                            onAddText = vm::addTextSource,
                            onCreateNote = vm::createNote,
                            onToggleSource = vm::toggleSourceSelected,
                            onRemoveSource = vm::removeSource,
                            onOpenSource = { s -> vm.openSourcePreview(if (state.activeSourcePreview?.id == s.id) null else s) },
                            onOpenPdf = vm::openPdfAtPage,
                            onBuildKnowledgeGraph = { s -> vm.createStudio("knowledge_graph", topic = s.title, sourceId = s.id) },
                            onAskTopic = { s, t -> vm.sendChat(String.format(L10n.t("main.explain_topic", en), t, s.title)) },
                            modifier = Modifier.width(300.dp),
                        )
                        ChatScreen(
                            state = state,
                            onSendChat = vm::sendChat,
                            onClickCitation = vm::openCitation,
                            onDismissCitation = vm::dismissCitation,
                            onOpenCitationPdf = { sid, pg ->
                                state.sources.firstOrNull { it.id == sid }?.let { vm.openPdfAtPage(it, pg) }
                            },
                            modifier = Modifier.weight(1f),
                        )
                        StudioScreen(
                            state = state,
                            onCreateStudio = { k, t, d, im, refs -> vm.createStudio(k, topic = t, detail = d, images = im, refArtifactIds = refs) },
                            onAddToSources = vm::addArtifactToSources,
                            onOpenArtifact = vm::openArtifact,
                            onToggleFullscreen = vm::toggleStudioFullscreen,
                            onRetryArtifact = vm::retryArtifact,
                            onRegenerateArtifact = vm::regenerateArtifact,
                            onApproveDraft = vm::approveDraft,
                        onReviseDraft = vm::reviseDraft,
                        onRefineNode = vm::refineNode,
                        core = vm.core,
                            onFeedback = vm::submitFeedback,
                            onQuizConcept = { c -> vm.createConceptLink("quiz", c) },
                            onExpandMindmap = { c -> vm.createConceptLink("mindmap", c) },
                            onAskConcept = { c -> vm.sendChat(c) },
                            modifier = Modifier.width(360.dp),
                        )
                    }
                } else {
                    when (selectedTab) {
                        0 -> SourcesScreen(
                            state = state,
                            onImportFile = vm::importFileUri,
                            onAddUrl = vm::addUrlSource,
                            onAddText = vm::addTextSource,
                            onCreateNote = vm::createNote,
                            onToggleSource = vm::toggleSourceSelected,
                            onRemoveSource = vm::removeSource,
                            onOpenSource = { s -> vm.openSourcePreview(if (state.activeSourcePreview?.id == s.id) null else s) },
                            onOpenPdf = vm::openPdfAtPage,
                            onBuildKnowledgeGraph = { s -> vm.createStudio("knowledge_graph", topic = s.title, sourceId = s.id); selectedTab = 2 },
                            onAskTopic = { s, t -> vm.sendChat(String.format(L10n.t("main.explain_topic", en), t, s.title)); selectedTab = 1 },
                            modifier = Modifier.weight(1f),
                        )
                        1 -> ChatScreen(
                            state = state,
                            onSendChat = vm::sendChat,
                            onClickCitation = vm::openCitation,
                            onDismissCitation = vm::dismissCitation,
                            onOpenCitationPdf = { sid, pg ->
                                state.sources.firstOrNull { it.id == sid }?.let { vm.openPdfAtPage(it, pg) }
                            },
                            modifier = Modifier.weight(1f),
                        )
                        else -> StudioScreen(
                            state = state,
                            onCreateStudio = { k, t, d, im, refs -> vm.createStudio(k, topic = t, detail = d, images = im, refArtifactIds = refs) },
                            onAddToSources = vm::addArtifactToSources,
                            onOpenArtifact = vm::openArtifact,
                            onToggleFullscreen = vm::toggleStudioFullscreen,
                            onRetryArtifact = vm::retryArtifact,
                            onRegenerateArtifact = vm::regenerateArtifact,
                            onApproveDraft = vm::approveDraft,
                        onReviseDraft = vm::reviseDraft,
                        onRefineNode = vm::refineNode,
                        core = vm.core,
                            onFeedback = vm::submitFeedback,
                            onQuizConcept = { c -> vm.createConceptLink("quiz", c) },
                            onExpandMindmap = { c -> vm.createConceptLink("mindmap", c) },
                            onAskConcept = { c -> vm.sendChat(c); selectedTab = 1 },
                            modifier = Modifier.weight(1f),
                        )
                    }
                }
            }

            // Animated Fullscreen Artifact Overlay (~190ms rapid expand/shrink)
            AnimatedVisibility(
                visible = state.studioFullscreen && state.activeArtifact != null,
                enter = if (state.prefs.fullscreenAnimation) scaleIn(tween(190), initialScale = 0.94f) + fadeIn(tween(160)) else fadeIn(tween(1)),
                exit = if (state.prefs.fullscreenAnimation) scaleOut(tween(170), targetScale = 0.94f) + fadeOut(tween(150)) else fadeOut(tween(1)),
            ) {
                Surface(modifier = Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                    StudioScreen(
                        state = state,
                        onCreateStudio = { k, t, d, im -> vm.createStudio(k, topic = t, detail = d, images = im) },
                        onOpenArtifact = vm::openArtifact,
                        onToggleFullscreen = vm::toggleStudioFullscreen,
                        onRetryArtifact = vm::retryArtifact,
                        onRegenerateArtifact = vm::regenerateArtifact,
                        onApproveDraft = vm::approveDraft,
                        onReviseDraft = vm::reviseDraft,
                        onRefineNode = vm::refineNode,
                        core = vm.core,
                        onFeedback = vm::submitFeedback,
                        onQuizConcept = { c -> vm.createConceptLink("quiz", c) },
                        onExpandMindmap = { c -> vm.createConceptLink("mindmap", c) },
                        onAskConcept = { c -> vm.toggleStudioFullscreen(); vm.sendChat(c); selectedTab = 1 },
                    )
                }
            }

            // Native PdfRenderer Fullscreen Overlay (direct page jump from citations/sources)
            state.pdfPreview?.let { target ->
                PdfViewerOverlay(target = target, onClose = vm::closePdfPreview)
            }
        }
    }

    if (state.showSettings) {
        UnifiedSettingsSheet(
            prefs = state.prefs,
            presets = state.providerPresets,
            profiles = state.modelProfiles,
            mediaRouting = state.mediaRouting,
            lectureRepairRounds = state.lectureRepairRounds,
            onSaveLectureRepairRounds = vm::saveLectureRepairRounds,
            tokenLimits = state.tokenLimits,
            onSaveTokenLimits = vm::saveTokenLimits,
            onResetTokenLimits = vm::resetTokenLimits,
            debugMode = state.debugMode,
            onToggleDebugMode = vm::setDebugMode,
            onExportLogs = vm::exportLogs,
            onSavePrefs = vm::updatePreferences,
            onSaveModel = vm::saveModelProfile,
            onDeleteProfile = vm::deleteModelProfile,
            onTestProfile = vm::testModelProfile,
            onClearKey = vm::clearModelKey,
            onSaveMediaRouting = vm::saveMediaRouting,
            onDownloadEmbedding = vm::downloadEmbeddingModel,
            onDismiss = { vm.setShowSettings(false) },
        )
    }
}
