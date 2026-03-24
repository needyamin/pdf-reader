import { contextBridge, ipcRenderer, IpcRendererEvent } from 'electron';
import * as path from 'path';

export interface PDFData {
    data: number[];
    fileName: string;
    filePath: string;
    fileSize: number;
}

// Resolve the pdf.js worker path
const pdfjsBasePath = path.join(__dirname, '..', 'node_modules', 'pdfjs-dist');
const pdfjsBuildPath = path.join(pdfjsBasePath, 'build');

contextBridge.exposeInMainWorld('electronAPI', {
    // File Operations
    openFileDialog: () => ipcRenderer.invoke('open-file-dialog'),
    savePDF: (data: number[], filePath: string) => ipcRenderer.invoke('save-pdf', data, filePath),
    savePDFAs: (data: number[]) => ipcRenderer.invoke('save-pdf-as', data),
    printPDF: (data?: number[]) => ipcRenderer.invoke('print-pdf', data),
    getRecentFiles: () => ipcRenderer.invoke('get-recent-files'),
    openRecentFile: (filePath: string) => ipcRenderer.invoke('open-recent-file', filePath),
    showFileInFolder: (filePath: string) => ipcRenderer.invoke('show-file-in-folder', filePath),

    // Window Controls
    minimizeWindow: () => ipcRenderer.invoke('window-minimize'),
    maximizeWindow: () => ipcRenderer.invoke('window-maximize'),
    closeWindow: () => ipcRenderer.invoke('window-close'),
    isMaximized: () => ipcRenderer.invoke('is-maximized'),
    toggleFullscreen: () => ipcRenderer.invoke('toggle-fullscreen'),

    // Theme
    getTheme: () => ipcRenderer.invoke('get-theme'),

    // Paths
    getPdfjsPath: () => pdfjsBuildPath,
    getPdfjsWorkerPath: () => path.join(pdfjsBuildPath, 'pdf.worker.min.js'),
    getPdfLibPath: () => path.join(__dirname, '..', 'node_modules', 'pdf-lib', 'dist', 'pdf-lib.min.js'),

    // Event Listeners
    onPDFLoaded: (callback: (data: PDFData) => void) => {
        const handler = (_event: IpcRendererEvent, data: PDFData) => callback(data);
        ipcRenderer.on('pdf-loaded', handler);
        return () => ipcRenderer.removeListener('pdf-loaded', handler);
    },

    onRecentFilesUpdated: (callback: (files: string[]) => void) => {
        const handler = (_event: IpcRendererEvent, files: string[]) => callback(files);
        ipcRenderer.on('recent-files-updated', handler);
        return () => ipcRenderer.removeListener('recent-files-updated', handler);
    },

    onWindowStateChanged: (callback: (state: string) => void) => {
        const handler = (_event: IpcRendererEvent, state: string) => callback(state);
        ipcRenderer.on('window-state-changed', handler);
        return () => ipcRenderer.removeListener('window-state-changed', handler);
    },

    onZoom: (callback: (action: string) => void) => {
        const handler = (_event: IpcRendererEvent, action: string) => callback(action);
        ipcRenderer.on('zoom', handler);
        return () => ipcRenderer.removeListener('zoom', handler);
    },
    onSavePDF: (callback: () => void) => {
        const handler = () => callback();
        ipcRenderer.on('save-pdf', handler);
        return () => ipcRenderer.removeListener('save-pdf', handler);
    },
    onSavePDFAs: (callback: () => void) => {
        const handler = () => callback();
        ipcRenderer.on('save-pdf-as', handler);
        return () => ipcRenderer.removeListener('save-pdf-as', handler);
    },
    onPrintPDF: (callback: () => void) => {
        const handler = () => callback();
        ipcRenderer.on('print-pdf', handler);
        return () => ipcRenderer.removeListener('print-pdf', handler);
    },
    onFocusSearch: (cb: () => void) => { const h = () => cb(); ipcRenderer.on('focus-search', h); return () => ipcRenderer.removeListener('focus-search', h); },
    onViewMode: (cb: (m: string) => void) => { const h = (_: unknown, m: string) => cb(m); ipcRenderer.on('view-mode', h); return () => ipcRenderer.removeListener('view-mode', h); },
    onToggleSidebar: (cb: () => void) => { const h = () => cb(); ipcRenderer.on('toggle-sidebar', h); return () => ipcRenderer.removeListener('toggle-sidebar', h); },
    onRotate: (cb: (deg: number) => void) => { const h = (_: unknown, d: number) => cb(d); ipcRenderer.on('rotate', h); return () => ipcRenderer.removeListener('rotate', h); },
    onShowInfo: (cb: () => void) => { const h = () => cb(); ipcRenderer.on('show-info', h); return () => ipcRenderer.removeListener('show-info', h); },
    onShowShortcuts: (cb: () => void) => { const h = () => cb(); ipcRenderer.on('show-shortcuts', h); return () => ipcRenderer.removeListener('show-shortcuts', h); },
    onShowAbout: (cb: () => void) => { const h = () => cb(); ipcRenderer.on('show-about', h); return () => ipcRenderer.removeListener('show-about', h); },
    onUpdateStatus: (cb: (msg: string) => void) => { const h = (_: unknown, m: string) => cb(m); ipcRenderer.on('update-status', h); return () => ipcRenderer.removeListener('update-status', h); },
});
