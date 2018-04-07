// app.js
//
// A plain JS file because I'm trying to avoid the deluge of heavy dependencies
// in the JavaScript ecosystem (WebPack, Browserify, etc.) for as long as
// possible. We're using lots of ES6 features here, but they're about 85%
// supported (by number of global installations) as of when I'm writing this,
// with the only really holdout being ... IE. Dynamic components are a
// relatively small part of this site and probably most visitors are part of
// that 85%, so I'm going to do this for now and see how long I can get away
// with it.
//
// One side effect is that we use `React.createElement` all over the place
// instead of the more familiar JSX. I nominally like the latter better, but
// not my *that* much, and am okay with this tradeoff for the time being.

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

//
// AccountPodcastSubscriptionToggler
//

class AccountPodcastSubscriptionToggler extends React.Component {
  constructor(props) {
    super(props);
    this.state = {
	  podcastId: props.podcastId,
	  subscribed: props.subscribed
    };

    // I tried arrow functions, but couldn't get them working for a class
    // function.
    this.handleClick = this.handleClick.bind(this);
  }

  handleClick(e) {
    e.preventDefault();
    console.log(`The link was clicked for: ${this.state.podcastId} w/ state: ${this.state.subscribed}`);
    this.setState(prevState => ({
      subscribed: !prevState.subscribed
    }));
  }

  render() {
    return React.createElement('a', {href: '#', onClick: this.handleClick},
      this.state.subscribed ? 'Unsubscribe' : 'Subscribe'
    );
  }
}

ReactDOM.render(
  React.createElement(AccountPodcastSubscriptionToggler, {podcastId: "1", subscribed: false}),
  document.getElementById('react-container')
);
