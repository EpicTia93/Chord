import { afterAll, expect, test } from "bun:test";
import { renderToString } from "react-dom/server";
import { ChordLayout, type ChordPackage } from "@chord/dev.improve.chord.lib.typeshare";

// The passive overlay only uses the viewport during server rendering. Effects
// that talk to Tauri do not run, and TauRPC needs the browser's Proxy constructor.
const previousWindow = globalThis.window;
globalThis.window = { innerHeight: 720, Proxy } as unknown as Window & typeof globalThis;
const { ChordsPageContent } = await import("#chords-page");
afterAll(() => {
  globalThis.window = previousWindow;
});

for (const layout of [ChordLayout.SidebarLeft, ChordLayout.FloatingBottom]) {
  test(`${layout} displays macros before arrow escapes when backslash is typed`, () => {
    const props: Parameters<typeof ChordsPageContent>[0] = {
      chordInputData: { input: ["Backslash"] },
      chordPanelData: { isVisible: true, layout },
      keyboardData: { isCapsPressed: false, isShiftPressed: false },
      frontmostData: { filePickerActive: false },
      chordPackageManagerData: { packages: [] },
      chordExecutionData: { loops: [], metaCommands: [], haltLoopTemplate: "0h{id}" },
      chordMacrosData: {
        entries: [
          {
            target: { type: "named", value: "window-size" },
            sequence: "1720x864",
            persistent: true,
          },
          { target: { type: "buffer", value: 1 }, sequence: "800x600", persistent: false },
        ],
      },
    };
    const html = renderToString(<ChordsPageContent {...props} />);
    expect(html).toContain("Escape suggestions");
    expect(html).toContain("1720x864");
    expect(html).toContain("800x600");
    expect(html.indexOf("\\1")).toBeLessThan(html.indexOf("\\=window-size="));
    expect(html.indexOf("\\=window-size=")).toBeLessThan(html.indexOf("\\up"));
  });

  for (const filePickerActive of [false, true]) {
    test(`${layout} follows native file-picker priority when active=${filePickerActive}`, () => {
      function packageFor(app: string, name: string): ChordPackage {
        const path = `chords/${app.replaceAll(".", "/")}/macos.toml`;
        return {
          name,
          rawChordsFiles: {},
          globalChords: [],
          compiledChordsFiles: {
            [path]: {
              name, pathslug: path, meta: {}, handlers: [],
              chords: [{ rawTrigger: "gh", trigger: { keys: ["KeyG", "KeyH"] }, name, index: 0, actions: [] }],
              chordHints: [{ rawPattern: "gh", pattern: { keys: ["KeyG", "KeyH"] }, description: name }],
            },
          },
        };
      }
      const html = renderToString(<ChordsPageContent
        chordInputData={{ input: ["KeyG", "KeyH"] }}
        chordPanelData={{ isVisible: true, layout }}
        keyboardData={{ isCapsPressed: false, isShiftPressed: false }}
        frontmostData={{ frontmostAppBundleId: "dev.fixture", filePickerActive }}
        chordPackageManagerData={{ packages: [packageFor("dev.fixture", "Host navigation"), packageFor("com.apple.finder", "Finder navigation")] }}
        chordExecutionData={{ loops: [], metaCommands: [], haltLoopTemplate: "0h{id}" }}
        chordMacrosData={{ entries: [] }}
      />);
      expect(html).toContain(filePickerActive ? "Finder navigation" : "Host navigation");
      expect(html).not.toContain(filePickerActive ? "Host navigation" : "Finder navigation");
    });
  }
}
