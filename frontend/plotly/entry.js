// The traces views use, and nothing else: plotly.js's own bundles leave out indicator or table, or
// carry every trace there is.
var Plotly = require("plotly.js/lib/core");
Plotly.register([
    require("plotly.js/lib/bar"),
    require("plotly.js/lib/pie"),
    require("plotly.js/lib/indicator"),
    require("plotly.js/lib/table"),
]);
window.Plotly = Plotly;
