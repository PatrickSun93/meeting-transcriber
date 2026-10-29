const path = require('path');
const HtmlWebpackPlugin = require('html-webpack-plugin');

module.exports = (env, argv) => ({
  entry: './src/client/index.js',
  output: {
    path: path.resolve(__dirname, 'src/client/build'),
    filename: 'bundle.[contenthash:8].js',
    publicPath: '/',
    clean: true
  },
  devtool: argv.mode === 'production' ? false : 'eval-source-map',
  module: {
    rules: [
      {
        test: /\.(js|jsx)$/,
        exclude: /node_modules/,
        use: {
          loader: 'babel-loader',
          options: {
            babelrc: false,
            presets: [
              ['@babel/preset-env', { targets: 'safari >= 15, chrome >= 100, edge >= 100' }],
              ['@babel/preset-react', { runtime: 'automatic' }]
            ]
          }
        }
      },
      {
        test: /\.css$/,
        use: ['style-loader', 'css-loader']
      }
    ]
  },
  plugins: [
    new HtmlWebpackPlugin({
      template: './public/index.html'
    })
  ],
  performance: {
    hints: false
  },
  resolve: {
    extensions: ['.js', '.jsx']
  }
});
