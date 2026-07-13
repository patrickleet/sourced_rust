export default {
	plugins: {
		'@csstools/postcss-global-data': {
			files: ['./src/custom-media.css']
		},
		'postcss-preset-env': {
			stage: 2,
			features: {
				'nesting-rules': true,
				'custom-media-queries': true,
				'media-query-ranges': true
			}
		}
	}
};
