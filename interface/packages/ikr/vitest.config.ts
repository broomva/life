import { defineConfig } from "vitest/config";
import { resolve } from "node:path";

export default defineConfig({
	resolve: {
		alias: {
			"@life/ikr-ir": resolve(__dirname, "../ir/src/index.ts"),
			"@life/ikr-signals": resolve(__dirname, "../signals/src/index.ts"),
			"@life/ikr-layout": resolve(__dirname, "../layout/src/index.ts"),
			"@life/ikr-policy": resolve(__dirname, "../policy/src/index.ts"),
			"@life/ikr-repair": resolve(__dirname, "../repair/src/index.ts"),
			"@life/ikr-render-dom": resolve(__dirname, "../render-dom/src/index.ts"),
			"@life/ikr-render-terminal": resolve(__dirname, "../render-terminal/src/index.ts"),
		},
	},
});
