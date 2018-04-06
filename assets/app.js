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
