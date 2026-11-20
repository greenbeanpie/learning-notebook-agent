// Renders plain text with $...$ / $$...$$ / \(...\) / \[...\] math. The host passes text, never HTML:
// everything is escaped first, so a message cannot inject markup or script.
function escapeHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}
window.lnaRender = function (text) {
  var root = document.getElementById('root');
  root.innerHTML = '<div style="white-space:pre-wrap">' + escapeHtml(String(text)) + '</div>';
  renderMathInElement(root, {
    delimiters: [
      { left: '$$', right: '$$', display: true },
      { left: '\\[', right: '\\]', display: true },
      { left: '\\(', right: '\\)', display: false },
      { left: '$', right: '$', display: false }
    ],
    throwOnError: false,
    trust: false
  });
  return root.querySelectorAll('.katex').length;
};
