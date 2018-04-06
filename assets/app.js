//import React from './react.production.min.js';
//import ReactDOM from './react-dom.production.min.js';

//import React from './react';
//import ReactDOM from './react-dom';

//const React = require('./react.production.min');
//const ReactDOM = require('./react-dom.production.min');

// Example of string interpolation:
//
// React.createElement('div', null, `Hello ${this.props.toWhat}`),

// Example of multiple child elements:
//
// React.createElement('div', null,
//     React.createElement(Greetings, { name : 'Chris' }),
//     React.createElement(Greetings, { name : 'Ming' }),
//     React.createElement(Greetings, { name : 'Joe' }),
// )

ReactDOM.render(
  React.createElement('div', null, `Hello, world!`),
  document.getElementById('react-container')
);
