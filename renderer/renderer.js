/* ==========================================
   LUMINA PDF READER - Renderer Process
   ========================================== */

let pdfjsLib = null;
let PDFLib = null;

// App State
const state = {
    pdfDoc: null,
    pdfData: null,
    currentPage: 1,
    totalPages: 0,
    scale: 1.0,
    viewMode: 'continuous',
    bookmarks: new Set(),
    renderedPages: new Map(),
    fileName: '',
    filePath: '',
    fileSize: 0,
    isLoading: false,
    sidebarVisible: false,
    renderingInProgress: false,
    pageBaseDims: new Map(),
    annotations: [],
    annTool: 'select',
    annColor: '#fef08a',
    pageRotations: {},
    pendingNote: null,
    formFields: [],
    formValues: {},
};

// DOM Elements cache
const el = {};

// ==========================================
// Initialization
// ==========================================

async function initApp() {
    // Load PDF.js dynamically
    const pdfjsPath = window.electronAPI.getPdfjsPath();
    const workerPath = window.electronAPI.getPdfjsWorkerPath();

    try {
        const fileUrl = 'file:///' + pdfjsPath.replace(/\\/g, '/') + '/pdf.min.js';

        await new Promise((resolve, reject) => {
            const script = document.createElement('script');
            script.src = fileUrl;
            script.onload = resolve;
            script.onerror = reject;
            document.head.appendChild(script);
        });

        pdfjsLib = window.pdfjsLib || globalThis.pdfjsLib;

        // Set worker source
        const workerUrl = 'file:///' + workerPath.replace(/\\/g, '/');
        pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

        console.log('PDF.js loaded successfully');

        try {
            const pdfLibPath = window.electronAPI.getPdfLibPath();
            const pdfLibUrl = 'file:///' + pdfLibPath.replace(/\\/g, '/');
            await new Promise((resolve, reject) => {
                const s = document.createElement('script');
                s.src = pdfLibUrl;
                s.onload = resolve;
                s.onerror = () => reject(new Error('Local load failed'));
                document.head.appendChild(s);
            });
            PDFLib = window.PDFLib;
        } catch (e) {
            try {
                await new Promise((resolve, reject) => {
                    const s = document.createElement('script');
                    s.src = 'https://unpkg.com/pdf-lib@1.17.1/dist/pdf-lib.min.js';
                    s.onload = resolve;
                    s.onerror = reject;
                    document.head.appendChild(s);
                });
                PDFLib = window.PDFLib;
            } catch (e2) {
                console.warn('pdf-lib load failed:', e2);
            }
        }
    } catch (err) {
        console.error('Failed to load PDF.js:', err);
        showToast('Failed to initialize PDF engine', 'error');
        return;
    }

    cacheElements();
    el.annColors?.[0]?.classList.add('active');
    loadTheme();
    bindEvents();
    setupDragAndDrop();
    loadBookmarks();
    setupKeyboardShortcuts();
}

function loadTheme() {
    const saved = localStorage.getItem('lumina-theme') || 'dark';
    applyTheme(saved);
}

function applyTheme(name) {
    if (name === 'dark') document.documentElement.removeAttribute('data-theme');
    else document.documentElement.setAttribute('data-theme', name);
    localStorage.setItem('lumina-theme', name);
    document.querySelectorAll('.theme-color').forEach(o => o.classList.toggle('active', o.dataset.theme === name));
}

function updateStatusBar() {
    if (el.statusZoom) el.statusZoom.textContent = `${Math.round(state.scale * 100)}%`;
    if (el.statusPages && state.pdfDoc) el.statusPages.textContent = `Page ${state.currentPage} of ${state.totalPages}`;
    if (el.statusText && state.pdfDoc) el.statusText.textContent = state.fileName;
}

function cacheElements() {
    const ids = [
        'titlebar-filename', 'btn-minimize', 'btn-maximize', 'btn-close',
        'btn-toggle-sidebar', 'sidebar',
        'btn-open', 'btn-prev-page', 'btn-next-page', 'page-input',
        'total-pages', 'search-input', 'search-results-count',
        'search-current', 'search-total', 'btn-search-prev',
        'btn-search-next', 'btn-search-close',
        'btn-zoom-in', 'btn-zoom-out', 'zoom-level', 'zoom-preset',
        'btn-single-page', 'btn-continuous',
        'btn-bookmark-page', 'btn-info',
        'welcome-screen', 'pdf-viewer', 'pages-container',
        'loading-overlay', 'loading-text', 'loading-bar',
        'thumbnails-container', 'bookmarks-list', 'bookmarks-empty',
        'outline-tree', 'outline-empty', 'drop-zone', 'btn-browse',
        'recent-section', 'recent-files-list',
        'toolbar-center', 'nav-controls', 'nav-separator', 'zoom-controls',
        'view-controls', 'view-separator',
        'file-info-modal', 'file-info-body', 'note-modal', 'note-input', 'note-ok', 'note-cancel', 'help-modal', 'help-modal-title', 'help-modal-body', 'toast-container',
        'annotation-toolbar', 'btn-save', 'btn-save-as', 'btn-print',
        'btn-rotate-ccw', 'btn-rotate-cw', 'comments-list', 'comments-empty',
        'forms-list', 'forms-empty', 'btn-fill-forms',
        'ann-color-custom',
        'statusbar', 'status-text', 'status-zoom', 'status-pages',
    ];

    ids.forEach(id => {
        const key = id.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
        el[key] = document.getElementById(id);
    });

    el.sidebarTabs = document.querySelectorAll('.sidebar-tab');
    el.panels = document.querySelectorAll('.panel');
    el.annTools = document.querySelectorAll('.ann-tool');
    el.annColors = document.querySelectorAll('.ann-color');
}

function bindEvents() {
    const api = window.electronAPI;

    // Titlebar
    el.btnMinimize.addEventListener('click', () => api.minimizeWindow());
    el.btnMaximize.addEventListener('click', () => api.maximizeWindow());
    el.btnClose.addEventListener('click', () => api.closeWindow());

    // Open file
    el.btnOpen.addEventListener('click', () => api.openFileDialog());
    el.btnBrowse.addEventListener('click', () => api.openFileDialog());
    el.btnSave?.addEventListener('click', () => savePDF());
    el.btnSaveAs?.addEventListener('click', () => savePDFAs());
    el.btnPrint?.addEventListener('click', () => printDocument());
    el.btnFillForms?.addEventListener('click', () => saveFormValues());
    el.noteOk?.addEventListener('click', () => {
        const p = state.pendingNote;
        if (p) {
            const text = (el.noteInput?.value || '').trim() || '(note)';
            const ann = { type: 'note', x: p.x, y: p.y, text, w: 0.05, h: 0.05 };
            ann.page = p.pageNum;
            ann.color = state.annColor;
            state.annotations.push(ann);
            redrawPageAnnotations(p.pageNum);
            updateCommentsPanel();
            state.pendingNote = null;
        }
        el.noteModal?.classList.add('hidden');
    });
    el.noteCancel?.addEventListener('click', () => {
        state.pendingNote = null;
        el.noteModal?.classList.add('hidden');
    });
    el.noteInput?.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') el.noteOk?.click();
        if (e.key === 'Escape') el.noteCancel?.click();
    });

    // Rotation
    el.btnRotateCcw?.addEventListener('click', () => rotatePage(-90));
    el.btnRotateCw?.addEventListener('click', () => rotatePage(90));

    // Annotation tools
    el.annTools?.forEach(btn => btn.addEventListener('click', () => setAnnTool(btn.dataset.tool)));
    el.annColors?.forEach(btn => btn.addEventListener('click', () => setAnnColor(btn.dataset.color)));
    el.annColorCustom?.addEventListener('input', (e) => setAnnColor(e.target.value));

    // Sidebar
    el.btnToggleSidebar.addEventListener('click', () => toggleSidebar());

    // Navigation
    el.btnPrevPage.addEventListener('click', () => goToPage(state.currentPage - 1));
    el.btnNextPage.addEventListener('click', () => goToPage(state.currentPage + 1));
    el.pageInput.addEventListener('change', (e) => {
        const page = parseInt(e.target.value, 10);
        if (page >= 1 && page <= state.totalPages) {
            goToPage(page);
        } else {
            e.target.value = state.currentPage;
        }
    });

    // Zoom
    el.btnZoomIn.addEventListener('click', () => zoomIn());
    el.btnZoomOut.addEventListener('click', () => zoomOut());
    el.zoomPreset.addEventListener('change', (e) => {
        const val = e.target.value;
        if (val === 'fit-width') fitWidth();
        else if (val === 'fit-page') fitPage();
        else setScale(parseFloat(val));
    });

    // View modes
    el.btnSinglePage.addEventListener('click', () => setViewMode('single'));
    el.btnContinuous.addEventListener('click', () => setViewMode('continuous'));

    // Bookmark & Info
    el.btnBookmarkPage.addEventListener('click', () => toggleBookmark(state.currentPage));
    el.btnInfo?.addEventListener('click', () => showFileInfo());

    // Search
    el.searchInput.addEventListener('input', onSearchInput);
    el.searchInput.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') e.shiftKey ? prevSearchMatch() : nextSearchMatch();
        if (e.key === 'Escape') clearSearch();
    });
    el.btnSearchPrev.addEventListener('click', prevSearchMatch);
    el.btnSearchNext.addEventListener('click', nextSearchMatch);
    el.btnSearchClose.addEventListener('click', clearSearch);

    // Sidebar tabs
    el.sidebarTabs.forEach(tab => {
        tab.addEventListener('click', () => switchTab(tab.dataset.tab));
    });

    // IPC Events
    api.onPDFLoaded(handlePDFLoaded);
    api.onSavePDF?.(() => savePDF());
    api.onSavePDFAs?.(() => savePDFAs());
    api.onPrintPDF?.(() => printDocument());
    api.onFocusSearch?.(() => el.searchInput?.focus());
    api.onViewMode?.(m => setViewMode(m));
    api.onToggleSidebar?.(toggleSidebar);
    api.onRotate?.(d => rotatePage(d));
    api.onShowInfo?.(showFileInfo);
    api.onShowShortcuts?.(showShortcutsModal);
    api.onShowAbout?.(showAboutModal);
    api.onUpdateStatus?.((msg) => { if (el.statusText) el.statusText.textContent = msg; });
    api.onRecentFilesUpdated(updateRecentFiles);
    api.onWindowStateChanged(updateWindowState);
    api.onZoom((action) => {
        if (action === 'in') zoomIn();
        else if (action === 'out') zoomOut();
        else if (action === 'reset') setScale(1);
    });

    // Modal close
    document.querySelectorAll('.modal-close').forEach(btn => btn.addEventListener('click', () => {
        el.fileInfoModal?.classList.add('hidden');
        el.helpModal?.classList.add('hidden');
        el.noteModal?.classList.add('hidden');
    }));
    document.querySelectorAll('.modal-backdrop').forEach(b => b.addEventListener('click', () => {
        el.fileInfoModal?.classList.add('hidden');
        el.helpModal?.classList.add('hidden');
        el.noteModal?.classList.add('hidden');
    }));

    // Scroll detection for current page in continuous mode
    el.pdfViewer?.addEventListener('scroll', onViewerScroll);

    // Menu bar - click to open, hover to switch, auto-close on mouse leave
    let menuOpen = false;
    const closeAllMenus = () => { menuOpen = false; document.querySelectorAll('.menu-item').forEach(m => m.classList.remove('open')); };
    document.querySelectorAll('.menu-item').forEach(item => {
        item.addEventListener('click', (e) => {
            if (e.target.closest('.menu-option')) return;
            e.stopPropagation();
            const wasOpen = item.classList.contains('open');
            closeAllMenus();
            if (!wasOpen) { item.classList.add('open'); menuOpen = true; }
        });
        item.addEventListener('mouseenter', () => {
            if (menuOpen) { document.querySelectorAll('.menu-item').forEach(m => m.classList.remove('open')); item.classList.add('open'); }
        });
    });
    document.getElementById('menubar')?.addEventListener('mouseleave', () => closeAllMenus());

    document.querySelectorAll('.menu-option[data-action]').forEach(opt => {
        opt.addEventListener('click', (e) => {
            e.stopPropagation();
            closeAllMenus();
            const a = opt.dataset.action;
            if (a === 'open') api.openFileDialog();
            else if (a === 'save') savePDF();
            else if (a === 'save-as') savePDFAs();
            else if (a === 'print') printDocument();
            else if (a === 'exit') api.closeWindow();
            else if (a === 'search') el.searchInput?.focus();
            else if (a.startsWith('tool-')) setAnnTool(a.replace('tool-', ''));
            else if (a === 'zoom-in') zoomIn();
            else if (a === 'zoom-out') zoomOut();
            else if (a === 'zoom-reset') setScale(1);
            else if (a === 'view-single') setViewMode('single');
            else if (a === 'view-continuous') setViewMode('continuous');
            else if (a === 'sidebar') toggleSidebar();
            else if (a === 'fullscreen') window.electronAPI.toggleFullscreen();
            else if (a === 'rotate-ccw') rotatePage(-90);
            else if (a === 'rotate-cw') rotatePage(90);
            else if (a === 'bookmark') toggleBookmark(state.currentPage);
            else if (a === 'info') showFileInfo();
            else if (a === 'shortcuts') showShortcutsModal();
            else if (a === 'about') showAboutModal();
            else if (a.startsWith('theme-')) applyTheme(a.replace('theme-', ''));
        });
    });
    // Theme color swatches
    document.querySelectorAll('.theme-color').forEach(btn => {
        btn.addEventListener('click', () => applyTheme(btn.dataset.theme));
    });

    document.addEventListener('click', (e) => {
        if (!e.target.closest('#menubar')) closeAllMenus();
    });
}

// ==========================================
// Drag and Drop
// ==========================================

function setupDragAndDrop() {
    const body = document.body;
    let dragCounter = 0;

    body.addEventListener('dragenter', (e) => {
        e.preventDefault();
        dragCounter++;
        body.classList.add('dragging-over');
        el.dropZone?.classList.add('drag-over');
    });

    body.addEventListener('dragover', (e) => {
        e.preventDefault();
    });

    body.addEventListener('dragleave', (e) => {
        e.preventDefault();
        dragCounter--;
        if (dragCounter <= 0) {
            dragCounter = 0;
            body.classList.remove('dragging-over');
            el.dropZone?.classList.remove('drag-over');
        }
    });

    body.addEventListener('drop', (e) => {
        e.preventDefault();
        dragCounter = 0;
        body.classList.remove('dragging-over');
        el.dropZone?.classList.remove('drag-over');

        const files = e.dataTransfer?.files;
        if (files && files.length > 0) {
            const file = files[0];
            if (file.name.toLowerCase().endsWith('.pdf')) {
                const reader = new FileReader();
                reader.onload = () => {
                    const uint8Array = new Uint8Array(reader.result);
                    handlePDFLoaded({
                        data: Array.from(uint8Array),
                        fileName: file.name,
                        filePath: file.path || file.name,
                        fileSize: file.size,
                    });
                };
                reader.readAsArrayBuffer(file);
            } else {
                showToast('Please drop a PDF file', 'error');
            }
        }
    });
}

// ==========================================
// Keyboard Shortcuts
// ==========================================

function setupKeyboardShortcuts() {
    document.addEventListener('keydown', (e) => {
        if (e.ctrlKey && e.key === 'o') {
            e.preventDefault();
            window.electronAPI.openFileDialog();
        }
        if (e.ctrlKey && e.key === 's') {
            e.preventDefault();
            if (state.pdfDoc) savePDF();
        }
        if (e.ctrlKey && e.key === 'p') {
            e.preventDefault();
            if (state.pdfDoc) window.electronAPI.printPDF();
        }
        if (e.ctrlKey && e.key === 'f') {
            e.preventDefault();
            el.searchInput?.focus();
        }
        if (e.key === 'Escape') {
            clearSearch();
            el.fileInfoModal?.classList.add('hidden');
            el.helpModal?.classList.add('hidden');
            el.noteModal?.classList.add('hidden');
            document.querySelectorAll('.menu-item').forEach(m => m.classList.remove('open'));
        }
        if (e.ctrlKey && e.key === 'b') {
            e.preventDefault();
            toggleSidebar();
        }
        if (e.ctrlKey && e.key === 'i' && state.pdfDoc) {
            e.preventDefault();
            showFileInfo();
        }
        if (e.ctrlKey && (e.key === '=' || e.key === '+')) {
            e.preventDefault();
            if (state.pdfDoc) zoomIn();
        }
        if (e.ctrlKey && e.key === '-') {
            e.preventDefault();
            if (state.pdfDoc) zoomOut();
        }
        if (e.ctrlKey && e.key === '0') {
            e.preventDefault();
            if (state.pdfDoc) setScale(1);
        }
        if (document.activeElement?.tagName !== 'INPUT') {
            if (e.key === 'ArrowLeft' || e.key === 'PageUp') {
                e.preventDefault(); goToPage(state.currentPage - 1);
            }
            if (e.key === 'ArrowRight' || e.key === 'PageDown') {
                e.preventDefault(); goToPage(state.currentPage + 1);
            }
            if (e.key === 'Home') {
                e.preventDefault(); goToPage(1);
            }
            if (e.key === 'End') {
                e.preventDefault(); goToPage(state.totalPages);
            }
        }
    });
}

// ==========================================
// PDF Loading & Rendering
// ==========================================

async function handlePDFLoaded(data) {
    if (!pdfjsLib) {
        showToast('PDF engine not ready', 'error');
        return;
    }

    showLoading('Opening document...');

    try {
        const typedArray = new Uint8Array(data.data);
        state.fileName = data.fileName;
        state.filePath = data.filePath;
        state.fileSize = data.fileSize;

        const loadingTask = pdfjsLib.getDocument({ data: typedArray });
        loadingTask.onProgress = (progress) => {
            if (progress.total > 0) {
                updateLoadingProgress(Math.round((progress.loaded / progress.total) * 50));
            }
        };

        state.pdfDoc = await loadingTask.promise;
        state.pdfData = data.data;
        state.totalPages = state.pdfDoc.numPages;
        state.currentPage = 1;
        state.annotations = [];
        state.pageRotations = {};
        loadBookmarks();

        // Update UI
        el.titlebarFilename.textContent = `— ${state.fileName}`;
        el.totalPages.textContent = state.totalPages;
        el.pageInput.max = state.totalPages;

        // Show elements
        el.welcomeScreen.classList.add('hidden');
        el.pdfViewer.classList.remove('hidden');
        el.navControls.classList.remove('hidden');
        el.navSeparator?.classList.remove('hidden');
        el.toolbarCenter.classList.remove('hidden');
        el.zoomControls.classList.remove('hidden');
        el.viewControls.classList.remove('hidden');
        el.viewSeparator?.classList.remove('hidden');
        el.annotationToolbar?.classList.remove('hidden');
        el.btnSave?.classList.remove('hidden');
        el.btnSaveAs?.classList.remove('hidden');
        el.btnPrint?.classList.remove('hidden');
        el.btnRotateCcw?.classList.remove('hidden');
        el.btnRotateCw?.classList.remove('hidden');

        // Fit width on load
        await calculateFitWidth();

        // Render all pages
        await renderAllPages();

        // Render thumbnails
        await renderThumbnails();

        // Load outline
        loadOutline();
        await loadFormFields();

        // Update bookmarks panel
        updateBookmarksPanel();
        updateCommentsPanel();
        updateFormsPanel();
        updatePageUI();

        hideLoading();
        showToast(`Opened "${state.fileName}" · ${state.totalPages} pages`, 'success');

        // Show sidebar
        if (!state.sidebarVisible) toggleSidebar();

    } catch (err) {
        console.error('Failed to load PDF:', err);
        hideLoading();
        showToast('Failed to load PDF document: ' + err.message, 'error');
    }
}

async function calculateFitWidth() {
    if (!state.pdfDoc) return;
    const page = await state.pdfDoc.getPage(1);
    const viewport = page.getViewport({ scale: 1 });
    const viewerWidth = el.pdfViewer.clientWidth - 60;
    state.scale = Math.round((viewerWidth / viewport.width) * 100) / 100;
    el.zoomLevel.textContent = `${Math.round(state.scale * 100)}%`;
}

async function renderAllPages() {
    const container = el.pagesContainer;
    container.innerHTML = '';
    state.renderedPages.clear();
    state.pageBaseDims.clear();
    state.renderingInProgress = true;

    try {
        // Pass 1: Create all page wrappers (fast, no pixel rendering)
        for (let i = 1; i <= state.totalPages; i++) {
            const page = await state.pdfDoc.getPage(i);
            const baseDims = page.getViewport({ scale: 1 });
            state.pageBaseDims.set(i, baseDims);

            const dw = baseDims.width * state.scale;
            const dh = baseDims.height * state.scale;

            const wrapper = document.createElement('div');
            wrapper.className = 'pdf-page-wrapper';
            wrapper.dataset.page = i;
            wrapper.style.width = `${dw}px`;
            wrapper.style.height = `${dh}px`;

            const canvas = document.createElement('canvas');
            canvas.style.width = `${dw}px`;
            canvas.style.height = `${dh}px`;

            const badge = document.createElement('div');
            badge.className = 'page-number-badge';
            badge.textContent = `${i} / ${state.totalPages}`;

            const overlay = document.createElement('canvas');
            overlay.className = 'annotation-overlay';
            overlay.dataset.page = i;
            overlay.width = Math.round(dw);
            overlay.height = Math.round(dh);

            wrapper.appendChild(canvas);
            wrapper.appendChild(overlay);
            wrapper.appendChild(badge);
            container.appendChild(wrapper);

            state.renderedPages.set(i, { canvas, wrapper, renderedAtScale: null });
            setupAnnotationOverlay(overlay, i);

            if (i % 20 === 0) {
                updateLoadingProgress(Math.round((i / state.totalPages) * 40));
                el.loadingText.textContent = `Preparing page ${i} of ${state.totalPages}...`;
                await new Promise(r => requestAnimationFrame(r));
            }
        }

        // Pass 2: Render only visible pages (fast perceived load)
        updateLoadingProgress(50);
        el.loadingText.textContent = 'Rendering visible pages...';
        await renderVisiblePagesNow();

        updateLoadingProgress(100);
        if (state.viewMode === 'single') applyViewMode();
    } finally {
        state.renderingInProgress = false;
    }
}

function getVisiblePageNums() {
    const viewer = el.pdfViewer;
    if (!viewer) return [...state.renderedPages.keys()];
    const vr = viewer.getBoundingClientRect();
    const buf = vr.height;
    const visible = [];
    for (const [i, { wrapper }] of state.renderedPages) {
        const r = wrapper.getBoundingClientRect();
        if (r.bottom >= vr.top - buf && r.top <= vr.bottom + buf) visible.push(i);
    }
    return visible.length > 0 ? visible : [1];
}

async function renderVisiblePagesNow() {
    const dpr = window.devicePixelRatio || 1;
    const renderScale = state.scale * dpr;
    const pages = getVisiblePageNums();

    for (const i of pages) {
        const entry = state.renderedPages.get(i);
        if (!entry || entry.renderedAtScale === state.scale) continue;

        const page = await state.pdfDoc.getPage(i);
        const viewport = page.getViewport({ scale: renderScale });
        const dv = page.getViewport({ scale: state.scale });

        const { canvas, wrapper } = entry;
        canvas.width = viewport.width;
        canvas.height = viewport.height;
        canvas.style.width = `${dv.width}px`;
        canvas.style.height = `${dv.height}px`;
        wrapper.style.width = `${dv.width}px`;
        wrapper.style.height = `${dv.height}px`;

        const ctx = canvas.getContext('2d', { willReadFrequently: true });
        await page.render({ canvasContext: ctx, viewport }).promise;
        entry.renderedAtScale = state.scale;

        const overlay = wrapper.querySelector('.annotation-overlay');
        if (overlay) {
            overlay.width = Math.round(dv.width);
            overlay.height = Math.round(dv.height);
        }
    }
}

async function loadFormFields() {
    if (!PDFLib || !state.pdfData) return;
    try {
        const pdfDoc = await PDFLib.PDFDocument.load(new Uint8Array(state.pdfData));
        const form = pdfDoc.getForm();
        const fields = form.getFields();
        const pages = pdfDoc.getPages();
        state.formFields = [];
        state.formValues = {};

        for (const f of fields) {
            const name = f.getName();
            const type = f.constructor.name;
            let value = '';
            try {
                if (type === 'PDFTextField') value = f.getText() || '';
                else if (type === 'PDFCheckBox') value = f.isChecked();
                else if (type === 'PDFDropdown') value = f.getSelected()?.[0] || '';
            } catch (_) {}
            state.formValues[name] = value;

            try {
                const widgets = f.acroField.getWidgets();
                for (const w of widgets) {
                    const rect = w.getRectangle();
                    let pageIdx = 0;
                    try {
                        const pageRef = w.P();
                        if (pageRef) {
                            const idx = pages.findIndex(p => p.ref === pageRef);
                            if (idx >= 0) pageIdx = idx;
                        }
                    } catch (_) {}
                    const pg = pages[pageIdx];
                    const { width: pw, height: ph } = pg.getSize();
                    if (rect.width > 0 && rect.height > 0) {
                        state.formFields.push({
                            name, type,
                            page: pageIdx + 1,
                            x: rect.x / pw,
                            y: 1 - (rect.y + rect.height) / ph,
                            w: rect.width / pw,
                            h: rect.height / ph,
                        });
                    }
                }
            } catch (_) {
                state.formFields.push({ name, type, page: 1, x: 0, y: 0, w: 0.2, h: 0.03 });
            }
        }
    } catch (e) {
        state.formFields = [];
        state.formValues = {};
    }
    renderInlineFormFields();
    if (state.formFields.length > 0) {
        const unique = new Set(state.formFields.map(f => f.name)).size;
        showToast(`Detected ${unique} form field${unique > 1 ? 's' : ''} — fill them directly on the page`, 'info');
    }
}

function cleanFieldName(name) {
    // "topmostSubform[0].Page2[0].TextField[3]" → "Text Field 3"
    let s = name.replace(/.*\./, '').replace(/\[\d+\]/g, '').replace(/([a-z])([A-Z])/g, '$1 $2').trim();
    return s || name;
}

function renderInlineFormFields() {
    // Remove old inline fields
    document.querySelectorAll('.inline-form-field').forEach(el => el.remove());
    if (!state.formFields.length) return;

    for (const f of state.formFields) {
        const entry = state.renderedPages.get(f.page);
        if (!entry) continue;
        const { wrapper } = entry;

        const el = document.createElement(f.type === 'PDFCheckBox' ? 'input' : 'input');
        el.className = 'inline-form-field';
        el.dataset.fieldName = f.name;
        el.style.position = 'absolute';
        el.style.left = `${f.x * 100}%`;
        el.style.top = `${f.y * 100}%`;
        el.style.width = `${f.w * 100}%`;
        el.style.height = `${f.h * 100}%`;
        el.style.zIndex = '3';

        if (f.type === 'PDFCheckBox') {
            el.type = 'checkbox';
            el.checked = !!state.formValues[f.name];
            el.addEventListener('change', () => { state.formValues[f.name] = el.checked; syncFormField(f.name); });
        } else {
            el.type = 'text';
            el.value = state.formValues[f.name] || '';
            el.placeholder = cleanFieldName(f.name);
            el.addEventListener('input', () => { state.formValues[f.name] = el.value; syncFormField(f.name); });
        }
        wrapper.appendChild(el);
    }
}

function syncFormField(name) {
    // Sync all inputs (inline + sidebar) with the same field name
    const val = state.formValues[name];
    document.querySelectorAll(`[data-field-name="${name}"]`).forEach(inp => {
        if (inp.type === 'checkbox') inp.checked = !!val;
        else if (inp.value !== val) inp.value = val || '';
    });
}

function updateFormsPanel() {
    const list = el.formsList, empty = el.formsEmpty, btn = el.btnFillForms;
    if (!list || !empty) return;
    if (state.formFields.length === 0) {
        empty.style.display = '';
        list.innerHTML = '';
        btn?.classList.add('hidden');
        return;
    }
    empty.style.display = 'none';
    btn?.classList.remove('hidden');
    list.innerHTML = '';
    const seen = new Set();
    state.formFields.filter(f => { if (seen.has(f.name)) return false; seen.add(f.name); return true; }).forEach(f => {
        const div = document.createElement('div');
        div.className = 'bookmark-item';
        div.style.flexDirection = 'column';
        div.style.alignItems = 'stretch';
        const label = document.createElement('label');
        label.textContent = cleanFieldName(f.name);
        label.title = f.name;
        label.style.fontSize = '11px';
        label.style.color = 'var(--text-tertiary)';
        let input;
        if (f.type === 'PDFCheckBox') {
            input = document.createElement('input');
            input.type = 'checkbox';
            input.checked = !!state.formValues[f.name];
            input.dataset.fieldName = f.name;
            input.addEventListener('change', () => { state.formValues[f.name] = input.checked; syncFormField(f.name); });
        } else {
            input = document.createElement('input');
            input.type = 'text';
            input.value = state.formValues[f.name] || '';
            input.dataset.fieldName = f.name;
            input.style.marginTop = '4px';
            input.style.padding = '6px';
            input.style.background = 'var(--bg-tertiary)';
            input.style.border = '1px solid var(--border-subtle)';
            input.style.borderRadius = '6px';
            input.style.color = 'var(--text-primary)';
            input.addEventListener('input', () => { state.formValues[f.name] = input.value; syncFormField(f.name); });
        }
        div.appendChild(label);
        div.appendChild(input);
        list.appendChild(div);
    });
}

async function saveFormValues() {
    if (!state.formFields.length) return;
    try {
        const bytes = await mergeFormValuesToPdf();
        state.pdfData = Array.from(bytes);
        showToast('Form values updated. Use Save to persist to file.', 'success');
        updateFormsPanel();
    } catch (e) {
        showToast('Failed: ' + e.message, 'error');
    }
}

async function mergeFormValuesToPdf() {
    if (!PDFLib) throw new Error('PDF library not loaded');
    const pdfDoc = await PDFLib.PDFDocument.load(new Uint8Array(state.pdfData));
    const form = pdfDoc.getForm();
    const fields = form.getFields();
    for (const f of fields) {
        const v = state.formValues[f.getName()];
        if (v === undefined) continue;
        try {
            if (f.constructor.name === 'PDFTextField') f.setText(String(v));
            else if (f.constructor.name === 'PDFCheckBox') v ? f.check() : f.uncheck();
        } catch (_) {}
    }
    return await pdfDoc.save();
}

function setupAnnotationOverlay(overlay, pageNum) {
    const entry = state.renderedPages.get(pageNum);
    const syncSize = () => {
        if (entry) {
            const w = entry.wrapper.offsetWidth, h = entry.wrapper.offsetHeight;
            if (w > 0 && h > 0 && (overlay.width !== w || overlay.height !== h)) {
                overlay.width = w;
                overlay.height = h;
                redrawPageAnnotations(pageNum);
            }
        }
    };
    requestAnimationFrame(() => syncSize());
    let isDrawing = false, startX, startY, points = [];

    const getCoords = (e) => {
        const rect = overlay.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return { x: 0, y: 0 };
        const scaleX = overlay.width / rect.width;
        const scaleY = overlay.height / rect.height;
        return { x: (e.clientX - rect.left) * scaleX, y: (e.clientY - rect.top) * scaleY };
    };

    const addAnnotation = (ann) => {
        ann.page = pageNum;
        ann.color = state.annColor;
        state.annotations.push(ann);
        redrawPageAnnotations(pageNum);
        updateCommentsPanel();
    };

    overlay.addEventListener('mousedown', (e) => {
        if (state.annTool === 'select') return;
        const { x, y } = getCoords(e);
        const ent = state.renderedPages.get(pageNum);
        if (state.annTool === 'eraser' && ent) {
            const w = ent.wrapper.offsetWidth, h = ent.wrapper.offsetHeight;
            const hit = hitTestAnnotation(pageNum, x / w, 1 - y / h);
            if (hit >= 0) {
                state.annotations.splice(hit, 1);
                redrawPageAnnotations(pageNum);
                updateCommentsPanel();
                showToast('Annotation removed', 'info');
            }
            return;
        }
        isDrawing = true;
        startX = x;
        startY = y;
        points = [{ x, y }];
    });

    overlay.addEventListener('mousemove', (e) => {
        if (!isDrawing) return;
        const { x, y } = getCoords(e);
        if (state.annTool === 'draw') {
            points.push({ x, y });
            drawTemp();
        }
    });

    overlay.addEventListener('mouseup', (e) => {
        if (!isDrawing) return;
        const { x, y } = getCoords(e);
        const entry = state.renderedPages.get(pageNum);
        if (!entry) return;
        const w = entry.wrapper.offsetWidth, h = entry.wrapper.offsetHeight;

        if (state.annTool === 'highlight' || state.annTool === 'underline' || state.annTool === 'strikeout') {
            const x1 = Math.min(startX, x), x2 = Math.max(startX, x);
            const y1 = Math.min(startY, y), y2 = Math.max(startY, y);
            const minSize = 3;
            if (Math.abs(x2 - x1) >= minSize || Math.abs(y2 - y1) >= minSize) {
                addAnnotation({ type: state.annTool, x: x1 / w, y: 1 - y2 / h, w: Math.max((x2 - x1) / w, 0.01), h: Math.max((y2 - y1) / h, 0.01) });
            }
        } else if (state.annTool === 'draw' && points.length >= 2) {
            const pts = points.map(p => ({ x: p.x / w, y: 1 - p.y / h }));
            addAnnotation({ type: 'draw', points: pts });
        } else if (state.annTool === 'note') {
            state.pendingNote = { x: x / w, y: 1 - y / h, pageNum };
            if (el.noteInput) el.noteInput.value = '';
            if (el.noteModal) {
                el.noteModal.classList.remove('hidden');
                el.noteInput?.focus();
            }
        }
        isDrawing = false;
        points = [];
        redrawPageAnnotations(pageNum);
    });

    overlay.addEventListener('mouseleave', () => { if (isDrawing) isDrawing = false; points = []; redrawPageAnnotations(pageNum); });
    overlay.addEventListener('mouseenter', syncSize);

    function drawTemp() {
        redrawPageAnnotations(pageNum);
        if (points.length > 1 && state.annTool === 'draw') {
            const ctx = overlay.getContext('2d', { willReadFrequently: false });
            ctx.strokeStyle = state.annColor;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(points[0].x, points[0].y);
            points.forEach(p => ctx.lineTo(p.x, p.y));
            ctx.stroke();
        }
    }
}

function redrawPageAnnotations(pageNum) {
    const overlay = document.querySelector(`.annotation-overlay[data-page="${pageNum}"]`);
    if (!overlay) return;
    const entry = state.renderedPages.get(pageNum);
    if (!entry) return;
    const w = entry.wrapper.offsetWidth, h = entry.wrapper.offsetHeight;
    if (w <= 0 || h <= 0) return;
    if (overlay.width !== w || overlay.height !== h) {
        overlay.width = w;
        overlay.height = h;
    }
    const ctx = overlay.getContext('2d', { willReadFrequently: false });
    ctx.clearRect(0, 0, w, h);
    const anns = state.annotations.filter(a => a.page === pageNum);
    anns.forEach(a => {
        if (a.type === 'highlight') {
            ctx.fillStyle = a.color + '80';
            ctx.fillRect(a.x * w, (1 - a.y - a.h) * h, a.w * w, a.h * h);
        } else if (a.type === 'underline' || a.type === 'strikeout') {
            ctx.strokeStyle = a.color;
            ctx.lineWidth = 2;
            ctx.beginPath();
            const y = a.type === 'underline' ? (1 - a.y) * h : (1 - a.y - a.h / 2) * h;
            ctx.moveTo(a.x * w, y);
            ctx.lineTo((a.x + a.w) * w, y);
            ctx.stroke();
        } else if (a.type === 'draw' && a.points) {
            ctx.strokeStyle = a.color;
            ctx.lineWidth = 2;
            ctx.beginPath();
            ctx.moveTo(a.points[0].x * w, (1 - a.points[0].y) * h);
            a.points.forEach(p => ctx.lineTo(p.x * w, (1 - p.y) * h));
            ctx.stroke();
        } else if (a.type === 'note') {
            ctx.fillStyle = '#fef08a';
            ctx.fillRect(a.x * w - 12, (1 - a.y) * h - 12, 24, 24);
            ctx.strokeStyle = '#333';
            ctx.strokeRect(a.x * w - 12, (1 - a.y) * h - 12, 24, 24);
        }
    });
}

function redrawAnnotations() {
    for (const [pageNum] of state.renderedPages) {
        redrawPageAnnotations(pageNum);
    }
}

function hitTestAnnotation(pageNum, nx, ny) {
    const anns = state.annotations.map((a, i) => ({ ...a, i })).filter(a => a.page === pageNum).reverse();
    for (const a of anns) {
        if (a.type === 'highlight' || a.type === 'underline' || a.type === 'strikeout') {
            if (nx >= a.x && nx <= a.x + a.w && ny >= a.y && ny <= a.y + a.h) return a.i;
        } else if (a.type === 'draw' && a.points?.length) {
            const pad = 0.02;
            const minX = Math.min(...a.points.map(p => p.x)) - pad, maxX = Math.max(...a.points.map(p => p.x)) + pad;
            const minY = Math.min(...a.points.map(p => p.y)) - pad, maxY = Math.max(...a.points.map(p => p.y)) + pad;
            if (nx >= minX && nx <= maxX && ny >= minY && ny <= maxY) return a.i;
        } else if (a.type === 'note') {
            const r = 0.03;
            if (Math.abs(nx - a.x) <= r && Math.abs(ny - a.y) <= r) return a.i;
        }
    }
    return -1;
}

function setAnnTool(tool) {
    state.annTool = tool;
    el.annTools?.forEach(b => b.classList.toggle('active', b.dataset.tool === tool));
    document.querySelectorAll('.annotation-overlay').forEach(o => {
        o.classList.toggle('ann-active', tool !== 'select');
        o.style.cursor = tool === 'eraser' ? 'cell' : tool === 'select' ? 'default' : 'crosshair';
    });
    redrawAnnotations();
}

function setAnnColor(color) {
    state.annColor = color;
    el.annColors?.forEach(b => b.classList.toggle('active', b.dataset.color === color));
    if (el.annColorCustom) el.annColorCustom.value = color;
}

async function savePDF() {
    if (!state.pdfDoc) return;
    if (!state.filePath || (!state.filePath.includes('\\') && !state.filePath.includes('/'))) {
        return savePDFAs();
    }
    try {
        showLoading('Saving...');
        const pdfBytes = PDFLib ? await mergeAnnotationsToPdf() : new Uint8Array(state.pdfData);
        const result = await window.electronAPI.savePDF(Array.from(pdfBytes), state.filePath);
        hideLoading();
        if (result?.ok) {
            showToast(PDFLib ? 'Saved successfully' : 'Saved (annotations not embedded)', 'success');
        } else showToast(result?.error || 'Save failed', 'error');
    } catch (e) {
        hideLoading();
        showToast('Save failed: ' + e.message, 'error');
    }
}

async function savePDFAs() {
    if (!state.pdfDoc) return;
    try {
        showLoading('Exporting current page...');

        // Render current page to a high-res canvas with annotations baked in
        const pageNum = state.currentPage;
        const page = await state.pdfDoc.getPage(pageNum);
        const dpr = 2;
        const viewport = page.getViewport({ scale: state.scale * dpr });
        const tmpCanvas = document.createElement('canvas');
        tmpCanvas.width = viewport.width;
        tmpCanvas.height = viewport.height;
        const ctx = tmpCanvas.getContext('2d');
        await page.render({ canvasContext: ctx, viewport }).promise;

        // Draw annotations on top
        const anns = state.annotations.filter(a => a.page === pageNum);
        const w = viewport.width, h = viewport.height;
        anns.forEach(a => {
            if (a.type === 'highlight') {
                ctx.fillStyle = a.color + '80';
                ctx.fillRect(a.x * w, (1 - a.y - a.h) * h, a.w * w, a.h * h);
            } else if (a.type === 'underline' || a.type === 'strikeout') {
                ctx.strokeStyle = a.color; ctx.lineWidth = 2 * dpr; ctx.beginPath();
                const y = a.type === 'underline' ? (1 - a.y) * h : (1 - a.y - a.h / 2) * h;
                ctx.moveTo(a.x * w, y); ctx.lineTo((a.x + a.w) * w, y); ctx.stroke();
            } else if (a.type === 'draw' && a.points) {
                ctx.strokeStyle = a.color; ctx.lineWidth = 2 * dpr; ctx.beginPath();
                ctx.moveTo(a.points[0].x * w, (1 - a.points[0].y) * h);
                a.points.forEach(p => ctx.lineTo(p.x * w, (1 - p.y) * h)); ctx.stroke();
            } else if (a.type === 'note') {
                ctx.fillStyle = '#fef08a';
                ctx.fillRect(a.x * w - 12 * dpr, (1 - a.y) * h - 12 * dpr, 24 * dpr, 24 * dpr);
                ctx.strokeStyle = '#333';
                ctx.strokeRect(a.x * w - 12 * dpr, (1 - a.y) * h - 12 * dpr, 24 * dpr, 24 * dpr);
            }
        });

        // Ask user where to save — dialog returns chosen extension
        // First try as PDF (single page), fallback prepared for image
        const pdfSinglePage = await exportCurrentPagePDF(pageNum);
        const result = await window.electronAPI.savePDFAs(Array.from(pdfSinglePage));
        hideLoading();

        if (result?.canceled) return;
        if (result?.ok) {
            const ext = result.ext || '.pdf';
            if (ext === '.png' || ext === '.jpg' || ext === '.jpeg') {
                // User chose image — re-save as image data
                const mime = ext === '.png' ? 'image/png' : 'image/jpeg';
                const dataUrl = tmpCanvas.toDataURL(mime, 0.95);
                const base64 = dataUrl.split(',')[1];
                const bin = atob(base64);
                const arr = new Uint8Array(bin.length);
                for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
                await window.electronAPI.savePDF(Array.from(arr), result.filePath);
                showToast(`Page ${pageNum} saved as ${ext.toUpperCase()} image`, 'success');
            } else {
                showToast(`Page ${pageNum} saved as PDF`, 'success');
            }
        } else {
            showToast(result?.error || 'Save failed', 'error');
        }
    } catch (e) {
        hideLoading();
        showToast('Save failed: ' + e.message, 'error');
    }
}

async function exportCurrentPagePDF(pageNum) {
    if (PDFLib) {
        const srcDoc = await PDFLib.PDFDocument.load(new Uint8Array(state.pdfData));
        const newDoc = await PDFLib.PDFDocument.create();
        const [copied] = await newDoc.copyPages(srcDoc, [pageNum - 1]);
        newDoc.addPage(copied);

        // Apply annotations for this page
        const page = newDoc.getPages()[0];
        const { width, height } = page.getSize();
        const anns = state.annotations.filter(a => a.page === pageNum);
        for (const ann of anns) {
            const rgb = hexToRgb(ann.color);
            if (ann.type === 'highlight') {
                page.drawRectangle({ x: ann.x * width, y: (1 - ann.y - ann.h) * height, width: ann.w * width, height: ann.h * height, color: PDFLib.rgb(rgb.r, rgb.g, rgb.b), opacity: 0.4 });
            } else if (ann.type === 'underline' || ann.type === 'strikeout') {
                const y = ann.type === 'underline' ? (1 - ann.y) * height : (1 - ann.y - ann.h / 2) * height;
                page.drawLine({ start: { x: ann.x * width, y }, end: { x: (ann.x + ann.w) * width, y }, thickness: 2, color: PDFLib.rgb(rgb.r, rgb.g, rgb.b) });
            } else if (ann.type === 'draw' && ann.points?.length > 1) {
                const pts = ann.points.map(p => ({ x: p.x * width, y: p.y * height }));
                for (let i = 1; i < pts.length; i++) page.drawLine({ start: pts[i - 1], end: pts[i], thickness: 2, color: PDFLib.rgb(rgb.r, rgb.g, rgb.b) });
            }
        }

        // Apply form values
        try {
            const form = newDoc.getForm();
            for (const f of form.getFields()) {
                const v = state.formValues[f.getName()];
                if (v === undefined) continue;
                try {
                    if (f.constructor.name === 'PDFTextField') f.setText(String(v));
                    else if (f.constructor.name === 'PDFCheckBox') v ? f.check() : f.uncheck();
                } catch (_) {}
            }
        } catch (_) {}

        return await newDoc.save();
    }
    return new Uint8Array(state.pdfData);
}

async function printDocument() {
    if (!state.pdfDoc) return;
    try {
        showLoading('Preparing page for print...');
        const pdfBytes = PDFLib ? await exportCurrentPagePDF(state.currentPage) : new Uint8Array(state.pdfData);
        hideLoading();
        await window.electronAPI.printPDF(Array.from(pdfBytes));
        showToast(`Page ${state.currentPage} opened — use Ctrl+P in the viewer to print`, 'info');
    } catch (e) {
        hideLoading();
        showToast('Print failed: ' + e.message, 'error');
    }
}

async function mergeAnnotationsToPdf() {
    const pdfDoc = await PDFLib.PDFDocument.load(new Uint8Array(state.pdfData));
    try {
        const form = pdfDoc.getForm();
        const fields = form.getFields();
        for (const f of fields) {
            const v = state.formValues[f.getName()];
            if (v === undefined) continue;
            try {
                if (f.constructor.name === 'PDFTextField') f.setText(String(v));
                else if (f.constructor.name === 'PDFCheckBox') v ? f.check() : f.uncheck();
            } catch (_) {}
        }
    } catch (_) {}
    const pages = pdfDoc.getPages();

    for (const ann of state.annotations) {
        const page = pages[ann.page - 1];
        if (!page) continue;
        const { width, height } = page.getSize();
        const rgb = hexToRgb(ann.color);

        if (ann.type === 'highlight') {
            page.drawRectangle({
                x: ann.x * width,
                y: (1 - ann.y - ann.h) * height,
                width: ann.w * width,
                height: ann.h * height,
                color: PDFLib.rgb(rgb.r, rgb.g, rgb.b),
                opacity: 0.4,
            });
        } else if (ann.type === 'underline' || ann.type === 'strikeout') {
            const y = ann.type === 'underline' ? (1 - ann.y) * height : (1 - ann.y - ann.h / 2) * height;
            page.drawLine({
                start: { x: ann.x * width, y },
                end: { x: (ann.x + ann.w) * width, y },
                thickness: 2,
                color: PDFLib.rgb(rgb.r, rgb.g, rgb.b),
            });
        } else if (ann.type === 'draw' && ann.points?.length > 1) {
            const pts = ann.points.map(p => ({ x: p.x * width, y: p.y * height }));
            for (let i = 1; i < pts.length; i++) {
                page.drawLine({
                    start: pts[i - 1],
                    end: pts[i],
                    thickness: 2,
                    color: PDFLib.rgb(rgb.r, rgb.g, rgb.b),
                });
            }
        } else if (ann.type === 'note') {
            page.drawRectangle({
                x: ann.x * width - 10,
                y: (1 - ann.y) * height - 10,
                width: 20,
                height: 20,
                color: PDFLib.rgb(1, 0.94, 0.54),
                borderColor: PDFLib.rgb(0.2, 0.2, 0.2),
            });
        }
    }
    return await pdfDoc.save();
}

function hexToRgb(hex) {
    const m = hex.match(/^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i);
    return m ? { r: parseInt(m[1], 16) / 255, g: parseInt(m[2], 16) / 255, b: parseInt(m[3], 16) / 255 } : { r: 1, g: 0.94, b: 0.33 };
}

function rotatePage(deg) {
    const rot = (state.pageRotations[state.currentPage] || 0) + deg;
    state.pageRotations[state.currentPage] = rot % 360;
    const entry = state.renderedPages.get(state.currentPage);
    if (entry) {
        entry.wrapper.style.transform = `rotate(${state.pageRotations[state.currentPage]}deg)`;
    }
}

function updateCommentsPanel() {
    const list = el.commentsList, empty = el.commentsEmpty;
    if (!list || !empty) return;
    if (state.annotations.length === 0) {
        empty.style.display = '';
        list.innerHTML = '';
        return;
    }
    empty.style.display = 'none';
    list.innerHTML = '';
    state.annotations.forEach((a, i) => {
        const div = document.createElement('div');
        div.className = 'bookmark-item';
        div.innerHTML = `<span class="bookmark-page">P${a.page}</span><span class="bookmark-label">${a.type}: ${a.text || ''}</span>`;
        div.addEventListener('click', () => goToPage(a.page));
        list.appendChild(div);
    });
}

let _reRenderQueued = false;

async function reRenderPages() {
    if (!state.pdfDoc) return;
    if (state.renderingInProgress) {
        _reRenderQueued = true;
        return;
    }

    state.renderingInProgress = true;
    try {
        await renderVisiblePagesNow();
        redrawAnnotations();
    } finally {
        state.renderingInProgress = false;
    }

    if (_reRenderQueued) {
        _reRenderQueued = false;
        requestAnimationFrame(() => reRenderPages());
    }
}

// ==========================================
// Navigation
// ==========================================

function goToPage(page) {
    if (!state.pdfDoc || page < 1 || page > state.totalPages) return;

    state.currentPage = page;
    updatePageUI();

    if (state.viewMode === 'single') {
        applyViewMode();
    }

    const wrapper = document.querySelector(`.pdf-page-wrapper[data-page="${page}"]`);
    if (wrapper) {
        wrapper.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
}

function updatePageUI() {
    el.pageInput.value = state.currentPage;

    document.querySelectorAll('.thumbnail-item').forEach(item => {
        item.classList.toggle('active', parseInt(item.dataset.page, 10) === state.currentPage);
    });

    const activeThumbnail = document.querySelector('.thumbnail-item.active');
    if (activeThumbnail) {
        activeThumbnail.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }

    updateBookmarkButton();
    updateStatusBar();
}

let _lazyRenderTimer = null;

function onViewerScroll() {
    if (!state.pdfDoc) return;

    if (state.viewMode === 'continuous') {
        const viewer = el.pdfViewer;
        const viewMid = viewer.scrollTop + viewer.clientHeight / 2;
        let closestPage = 1, closestDist = Infinity;

        document.querySelectorAll('.pdf-page-wrapper').forEach(wrapper => {
            const dist = Math.abs(viewMid - (wrapper.offsetTop + wrapper.offsetHeight / 2));
            if (dist < closestDist) {
                closestDist = dist;
                closestPage = parseInt(wrapper.dataset.page, 10);
            }
        });

        if (closestPage !== state.currentPage) {
            state.currentPage = closestPage;
            updatePageUI();
        }
    }

    // Lazy render pages scrolled into view
    clearTimeout(_lazyRenderTimer);
    _lazyRenderTimer = setTimeout(() => {
        if (!state.renderingInProgress) renderVisiblePagesNow().then(() => redrawAnnotations());
    }, 150);
}

// ==========================================
// Zoom
// ==========================================

function zoomIn() { setScale(Math.min(state.scale + 0.25, 5)); }
function zoomOut() { setScale(Math.max(state.scale - 0.25, 0.25)); }

let _zoomTimer = null;

function setScale(newScale) {
    state.scale = newScale;
    el.zoomLevel.textContent = `${Math.round(newScale * 100)}%`;
    updateStatusBar();

    const options = el.zoomPreset.querySelectorAll('option');
    let matched = false;
    options.forEach(opt => {
        if (parseFloat(opt.value) === newScale) {
            el.zoomPreset.value = opt.value;
            matched = true;
        }
    });
    if (!matched) el.zoomPreset.value = '1';

    // Instant CSS resize for immediate visual feedback
    for (const [pageNum, { canvas, wrapper }] of state.renderedPages) {
        const base = state.pageBaseDims.get(pageNum);
        if (!base) continue;
        const w = base.width * newScale, h = base.height * newScale;
        wrapper.style.width = `${w}px`;
        wrapper.style.height = `${h}px`;
        canvas.style.width = `${w}px`;
        canvas.style.height = `${h}px`;
    }

    // Debounced pixel-perfect re-render
    clearTimeout(_zoomTimer);
    _zoomTimer = setTimeout(() => reRenderPages(), 200);
}

async function fitWidth() {
    if (!state.pdfDoc) return;
    const page = await state.pdfDoc.getPage(1);
    const viewport = page.getViewport({ scale: 1 });
    const viewerWidth = el.pdfViewer.clientWidth - 60;
    setScale(Math.round((viewerWidth / viewport.width) * 100) / 100);
    el.zoomPreset.value = 'fit-width';
}

async function fitPage() {
    if (!state.pdfDoc) return;
    const page = await state.pdfDoc.getPage(1);
    const viewport = page.getViewport({ scale: 1 });
    const viewerWidth = el.pdfViewer.clientWidth - 60;
    const viewerHeight = el.pdfViewer.clientHeight - 60;
    const newScale = Math.min(viewerWidth / viewport.width, viewerHeight / viewport.height);
    setScale(Math.round(newScale * 100) / 100);
    el.zoomPreset.value = 'fit-page';
}

// ==========================================
// View Modes
// ==========================================

function setViewMode(mode) {
    state.viewMode = mode;
    el.btnSinglePage.classList.toggle('active', mode === 'single');
    el.btnContinuous.classList.toggle('active', mode === 'continuous');
    applyViewMode();
}

function applyViewMode() {
    document.querySelectorAll('.pdf-page-wrapper').forEach(wrapper => {
        const pageNum = parseInt(wrapper.dataset.page, 10);
        if (state.viewMode === 'single') {
            wrapper.style.display = pageNum === state.currentPage ? '' : 'none';
        } else {
            wrapper.style.display = '';
        }
    });
}

// ==========================================
// Thumbnails
// ==========================================

async function renderThumbnails() {
    const container = el.thumbnailsContainer;
    if (!container) return;
    container.innerHTML = '';

    for (let i = 1; i <= state.totalPages; i++) {
        const page = await state.pdfDoc.getPage(i);
        const viewport = page.getViewport({ scale: 0.25 });

        const item = document.createElement('div');
        item.className = 'thumbnail-item';
        item.dataset.page = i;
        if (i === state.currentPage) item.classList.add('active');

        const canvasWrapper = document.createElement('div');
        canvasWrapper.className = 'thumbnail-canvas-wrapper';

        const canvas = document.createElement('canvas');
        canvas.width = viewport.width;
        canvas.height = viewport.height;

        const pageNum = document.createElement('div');
        pageNum.className = 'thumbnail-page-num';
        pageNum.textContent = i;

        canvasWrapper.appendChild(canvas);
        item.appendChild(canvasWrapper);
        item.appendChild(pageNum);
        container.appendChild(item);

        item.addEventListener('click', () => goToPage(i));

        const ctx = canvas.getContext('2d', { willReadFrequently: true });
        await page.render({ canvasContext: ctx, viewport }).promise;
    }
    updatePageUI();
}

// ==========================================
// Bookmarks
// ==========================================

function getBookmarksKey() {
    return state.filePath ? `lumina-bm-${state.filePath.replace(/[^a-zA-Z0-9]/g, '_').slice(-80)}` : 'lumina-bookmarks';
}

function loadBookmarks() {
    try {
        const stored = localStorage.getItem(getBookmarksKey());
        if (stored) state.bookmarks = new Set(JSON.parse(stored));
        else state.bookmarks = new Set();
    } catch { state.bookmarks = new Set(); }
}

function saveBookmarks() {
    localStorage.setItem(getBookmarksKey(), JSON.stringify([...state.bookmarks]));
}

function toggleBookmark(page) {
    if (state.bookmarks.has(page)) {
        state.bookmarks.delete(page);
        showToast(`Removed bookmark from page ${page}`, 'info');
    } else {
        state.bookmarks.add(page);
        showToast(`Bookmarked page ${page}`, 'success');
    }
    saveBookmarks();
    updateBookmarksPanel();
    updateBookmarkButton();
}

function updateBookmarkButton() {
    const btn = el.btnBookmarkPage;
    if (!btn) return;
    const isBookmarked = state.bookmarks.has(state.currentPage);
    const svgPath = btn.querySelector('svg path');
    if (svgPath) svgPath.setAttribute('fill', isBookmarked ? 'currentColor' : 'none');
    btn.classList.toggle('active', isBookmarked);
}

function updateBookmarksPanel() {
    const list = el.bookmarksList;
    const empty = el.bookmarksEmpty;
    if (!list || !empty) return;

    if (state.bookmarks.size === 0) {
        empty.style.display = '';
        list.innerHTML = '';
        return;
    }

    empty.style.display = 'none';
    list.innerHTML = '';

    [...state.bookmarks].sort((a, b) => a - b).forEach(page => {
        const item = document.createElement('div');
        item.className = 'bookmark-item';

        const pageLabel = document.createElement('span');
        pageLabel.className = 'bookmark-page';
        pageLabel.textContent = `P${page}`;

        const label = document.createElement('span');
        label.className = 'bookmark-label';
        label.textContent = `Page ${page}`;

        const removeBtn = document.createElement('button');
        removeBtn.className = 'bookmark-remove';
        removeBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 6L6 18M6 6l12 12"/></svg>';
        removeBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            toggleBookmark(page);
        });

        item.appendChild(pageLabel);
        item.appendChild(label);
        item.appendChild(removeBtn);
        list.appendChild(item);

        item.addEventListener('click', () => goToPage(page));
    });
}

// ==========================================
// Document Outline
// ==========================================

async function loadOutline() {
    if (!state.pdfDoc) return;
    try {
        const outline = await state.pdfDoc.getOutline();
        if (!outline || outline.length === 0) {
            el.outlineEmpty.style.display = '';
            el.outlineTree.innerHTML = '';
            return;
        }
        el.outlineEmpty.style.display = 'none';
        el.outlineTree.innerHTML = '';
        await renderOutlineItems(outline, el.outlineTree, 1);
    } catch (err) {
        console.error('Failed to load outline:', err);
    }
}

async function renderOutlineItems(items, container, level) {
    for (const item of items) {
        const div = document.createElement('div');
        div.className = `outline-item level-${Math.min(level, 3)}`;
        div.textContent = item.title;

        if (item.dest) {
            try {
                let dest = item.dest;
                if (typeof dest === 'string') dest = await state.pdfDoc.getDestination(dest);
                if (dest) {
                    const ref = dest[0];
                    const pageIndex = await state.pdfDoc.getPageIndex(ref);
                    div.addEventListener('click', () => goToPage(pageIndex + 1));
                }
            } catch (err) { /* skip invalid */ }
        }

        container.appendChild(div);

        if (item.items && item.items.length > 0) {
            await renderOutlineItems(item.items, container, level + 1);
        }
    }
}

// ==========================================
// Search
// ==========================================

let searchTimeout = null;
let searchResults = [];
let currentSearchIdx = -1;

async function onSearchInput(e) {
    const query = e.target.value.trim();
    clearTimeout(searchTimeout);

    if (query.length < 2) {
        hideSearchControls();
        return;
    }

    showSearchControls();

    searchTimeout = setTimeout(async () => {
        searchResults = [];
        currentSearchIdx = -1;

        for (let i = 1; i <= state.totalPages; i++) {
            const page = await state.pdfDoc.getPage(i);
            const textContent = await page.getTextContent();
            const text = textContent.items.map(item => item.str).join(' ');

            if (text.toLowerCase().includes(query.toLowerCase())) {
                searchResults.push(i);
            }
        }

        el.searchTotal.textContent = searchResults.length;
        if (searchResults.length > 0) {
            currentSearchIdx = 0;
            el.searchCurrent.textContent = 1;
            goToPage(searchResults[0]);
        } else {
            el.searchCurrent.textContent = 0;
        }
    }, 300);
}

function nextSearchMatch() {
    if (searchResults.length === 0) return;
    currentSearchIdx = (currentSearchIdx + 1) % searchResults.length;
    el.searchCurrent.textContent = currentSearchIdx + 1;
    goToPage(searchResults[currentSearchIdx]);
}

function prevSearchMatch() {
    if (searchResults.length === 0) return;
    currentSearchIdx = (currentSearchIdx - 1 + searchResults.length) % searchResults.length;
    el.searchCurrent.textContent = currentSearchIdx + 1;
    goToPage(searchResults[currentSearchIdx]);
}

function showSearchControls() {
    el.btnSearchPrev?.classList.remove('hidden');
    el.btnSearchNext?.classList.remove('hidden');
    el.btnSearchClose?.classList.remove('hidden');
    el.searchResultsCount?.classList.remove('hidden');
}

function hideSearchControls() {
    el.btnSearchPrev?.classList.add('hidden');
    el.btnSearchNext?.classList.add('hidden');
    el.btnSearchClose?.classList.add('hidden');
    el.searchResultsCount?.classList.add('hidden');
    searchResults = [];
    currentSearchIdx = -1;
}

function clearSearch() {
    el.searchInput.value = '';
    hideSearchControls();
}

// ==========================================
// Sidebar
// ==========================================

function toggleSidebar() {
    state.sidebarVisible = !state.sidebarVisible;
    el.sidebar.classList.toggle('sidebar-collapsed', !state.sidebarVisible);
    el.btnToggleSidebar.classList.toggle('active', state.sidebarVisible);
}

function switchTab(tabName) {
    el.sidebarTabs.forEach(tab => {
        tab.classList.toggle('active', tab.dataset.tab === tabName);
    });
    el.panels.forEach(panel => {
        panel.classList.toggle('active', panel.id === `panel-${tabName}`);
    });
}

// ==========================================
// Recent Files
// ==========================================

function updateRecentFiles(files) {
    if (!files || files.length === 0) {
        el.recentSection?.classList.add('hidden');
        return;
    }

    el.recentSection?.classList.remove('hidden');
    const list = el.recentFilesList;
    if (!list) return;
    list.innerHTML = '';

    files.slice(0, 8).forEach(filePath => {
        const parts = filePath.replace(/\\/g, '/').split('/');
        const fileName = parts.pop();
        const dirPath = parts.join('/');

        const item = document.createElement('div');
        item.className = 'recent-file-item';
        item.innerHTML = `
      <div class="recent-file-icon">
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/>
          <path d="M14 2v6h6"/>
        </svg>
      </div>
      <div class="recent-file-info">
        <div class="recent-file-name">${fileName}</div>
        <div class="recent-file-path">${dirPath}</div>
      </div>
    `;
        item.addEventListener('click', () => window.electronAPI.openRecentFile(filePath));
        list.appendChild(item);
    });
}

// ==========================================
// File Info Modal
// ==========================================

async function showFileInfo() {
    if (!state.pdfDoc) return;

    const metadata = await state.pdfDoc.getMetadata();
    const info = metadata?.info || {};

    const formatSize = (bytes) => {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    };

    const body = el.fileInfoBody;
    body.innerHTML = '';

    const properties = [
        ['File Name', state.fileName],
        ['File Path', state.filePath],
        ['File Size', formatSize(state.fileSize)],
        ['Pages', state.totalPages],
        ['Title', info.Title || '—'],
        ['Author', info.Author || '—'],
        ['Subject', info.Subject || '—'],
        ['Creator', info.Creator || '—'],
        ['Producer', info.Producer || '—'],
    ];

    properties.forEach(([label, value]) => {
        const row = document.createElement('div');
        row.className = 'info-row';
        row.innerHTML = `
      <span class="info-label">${label}</span>
      <span class="info-value">${value}</span>
    `;
        body.appendChild(row);
    });

    el.fileInfoModal.classList.remove('hidden');
}

function showShortcutsModal() {
    el.helpModalTitle.textContent = 'Keyboard Shortcuts';
    el.helpModalBody.innerHTML = `
      <div class="info-row"><span>Ctrl+O</span><span>Open PDF</span></div>
      <div class="info-row"><span>Ctrl+S</span><span>Save</span></div>
      <div class="info-row"><span>Ctrl+P</span><span>Print</span></div>
      <div class="info-row"><span>Ctrl+F</span><span>Search</span></div>
      <div class="info-row"><span>Ctrl+B</span><span>Toggle Sidebar</span></div>
      <div class="info-row"><span>Ctrl+I</span><span>Document Info</span></div>
      <div class="info-row"><span>Ctrl+/-</span><span>Zoom In/Out</span></div>
      <div class="info-row"><span>Ctrl+0</span><span>Reset Zoom</span></div>
      <div class="info-row"><span>Arrow/PageUp/Down</span><span>Navigate</span></div>
      <div class="info-row"><span>F11</span><span>Full Screen</span></div>
    `;
    el.helpModal.classList.remove('hidden');
}

function showAboutModal() {
    el.helpModalTitle.textContent = 'About Advanced PDF Reader';
    el.helpModalBody.innerHTML = `
      <p style="margin-bottom:12px"><strong>Advanced PDF Reader</strong> v1.0.0</p>
      <p style="color:var(--text-secondary);font-size:12px;margin-bottom:8px">Professional PDF viewer with annotations, form filling, themes, and more.</p>
      <p style="color:var(--text-tertiary);font-size:11px">By YAMiN HOSSAIN &bull; <a href="https://github.com/needyamin/pdf-reader" style="color:var(--accent-primary)">GitHub</a></p>
    `;
    el.helpModal.classList.remove('hidden');
}

// ==========================================
// Loading UI
// ==========================================

function showLoading(text = 'Loading...') {
    state.isLoading = true;
    el.loadingOverlay.classList.remove('hidden');
    el.loadingText.textContent = text;
    el.loadingBar.style.width = '0%';
}

function hideLoading() {
    state.isLoading = false;
    el.loadingBar.style.width = '100%';
    setTimeout(() => el.loadingOverlay.classList.add('hidden'), 300);
}

function updateLoadingProgress(pct) {
    el.loadingBar.style.width = `${pct}%`;
}

// ==========================================
// Toast Notifications
// ==========================================

function showToast(message, type = 'info') {
    const container = el.toastContainer;
    if (!container) return;

    const toast = document.createElement('div');
    toast.className = `toast toast-${type}`;

    const icons = {
        success: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#34d399" stroke-width="2"><path d="M20 6L9 17l-5-5"/></svg>',
        error: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#f87171" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M15 9l-6 6M9 9l6 6"/></svg>',
        info: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="#60a5fa" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 16v-4M12 8h.01"/></svg>',
    };

    toast.innerHTML = `
    <div class="toast-icon">${icons[type] || icons.info}</div>
    <div class="toast-message">${message}</div>
  `;

    container.appendChild(toast);
    setTimeout(() => {
        toast.classList.add('toast-out');
        setTimeout(() => toast.remove(), 300);
    }, 3500);
}

// ==========================================
// Window State
// ==========================================

function updateWindowState(windowState) {
    const btn = el.btnMaximize;
    if (!btn) return;
    const isMax = windowState === 'maximized';
    btn.title = isMax ? 'Restore' : 'Maximize';
    btn.innerHTML = isMax
        ? '<svg width="12" height="12" viewBox="0 0 12 12"><rect x="2" y="0" width="9" height="9" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/><rect x="0" y="2" width="9" height="9" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/></svg>'
        : '<svg width="12" height="12" viewBox="0 0 12 12"><rect x="1" y="1" width="10" height="10" rx="1.5" stroke="currentColor" stroke-width="1.3" fill="none"/></svg>';
}

// ==========================================
// Start the app
// ==========================================
initApp();
