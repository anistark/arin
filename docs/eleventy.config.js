import fs from "node:fs";
import path from "node:path";
import { HtmlBasePlugin } from "@11ty/eleventy";
import syntaxHighlight from "@11ty/eleventy-plugin-syntaxhighlight";
import markdownItAnchor from "markdown-it-anchor";

const docsDir = import.meta.dirname;

const DOCS = [
  {
    file: "building.md",
    slug: "building",
    title: "Building",
    description: "Build Arin from source and run the daemon.",
  },
  {
    file: "cli.md",
    slug: "cli",
    title: "CLI",
    description: "Drive Arin from a shell, the quickest way to see it work.",
  },
  {
    file: "mcp.md",
    slug: "mcp",
    title: "MCP",
    description: "Connect Arin to any agent that speaks MCP.",
  },
  {
    file: "nix.md",
    slug: "nix",
    title: "Nix",
    description: "Install Arin with Nix, and start it at login with nix-darwin.",
  },
  {
    file: "protocol.md",
    slug: "protocol",
    title: "Protocol",
    description: "Newline-delimited JSON over a Unix domain socket.",
  },
  {
    file: "resolvers.md",
    slug: "resolvers",
    title: "Resolvers",
    description: "Turn a phrase like “the Submit button” into coordinates.",
  },
];

export default function (eleventyConfig) {
  eleventyConfig.addPlugin(HtmlBasePlugin);
  eleventyConfig.addPlugin(syntaxHighlight);

  eleventyConfig.amendLibrary("md", (md) => {
    md.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.headerLink({ safariReaderFix: true }),
      level: [2, 3],
    });
  });

  eleventyConfig.addPassthroughCopy({ "../assets": "assets/brand" });

  eleventyConfig.addGlobalData("docsNav", DOCS);
  eleventyConfig.addGlobalData("site", {
    name: "Arin",
    tagline: "An annotation layer any agent can draw on.",
    repo: "https://github.com/anistark/arin",
  });

  for (const doc of DOCS) {
    const raw = fs.readFileSync(path.join(docsDir, doc.file), "utf8");
    eleventyConfig.addTemplate(`docs/${doc.slug}.md`, raw, {
      layout: "layouts/docs.njk",
      title: doc.title,
      description: doc.description,
      slug: doc.slug,
      permalink: `/docs/${doc.slug}/`,
    });
  }

  eleventyConfig.addWatchTarget("./*.md");
  eleventyConfig.addWatchTarget("src/styles/");

  eleventyConfig.setServerOptions({
    showAllHosts: Boolean(process.env.DOCS_SHOW_HOSTS),
  });

  return {
    dir: {
      input: "src",
      output: "_site",
    },
    pathPrefix: process.env.PATH_PREFIX || "/",
    markdownTemplateEngine: false,
  };
}
