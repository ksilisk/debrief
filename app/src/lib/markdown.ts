// A small Markdown renderer for summaries: headings, lists, paragraphs, bold,
// italics and inline code. Everything is escaped first, so the summary text
// can't add its own HTML to the page.

function escape(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function inline(text: string): string {
  return escape(text)
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/(^|[^*\w])\*([^*\s][^*]*)\*/g, "$1<em>$2</em>");
}

export function renderMarkdown(source: string): string {
  const html: string[] = [];
  let paragraph: string[] = [];
  let list: "ul" | "ol" | null = null;

  const endParagraph = () => {
    if (paragraph.length > 0) html.push(`<p>${inline(paragraph.join(" "))}</p>`);
    paragraph = [];
  };
  const endList = () => {
    if (list) html.push(`</${list}>`);
    list = null;
  };

  for (const raw of source.replace(/\r\n?/g, "\n").split("\n")) {
    const line = raw.trim();
    const heading = /^#{1,6}\s+(.*?)\s*#*$/.exec(line);
    const item = /^(?:[-*+]|(\d+)[.)])\s+(.*)$/.exec(line);
    if (line === "" || /^(-{3,}|\*{3,}|_{3,})$/.test(line)) {
      endParagraph();
      endList();
    } else if (heading) {
      endParagraph();
      endList();
      html.push(`<h3>${inline(heading[1])}</h3>`);
    } else if (item) {
      endParagraph();
      const kind = item[1] === undefined ? "ul" : "ol";
      if (list !== kind) {
        endList();
        html.push(`<${kind}>`);
        list = kind;
      }
      html.push(`<li>${inline(item[2])}</li>`);
    } else {
      endList();
      paragraph.push(line);
    }
  }
  endParagraph();
  endList();
  return html.join("");
}
