    class Logger < Middleware
      def initialize(app, logger = nil, options = {})
        super(app)
        logger ||= ::Logger.new($stdout)
        formatter_class = options.delete(:formatter) || Logging::Formatter
        @formatter = formatter_class.new(logger: logger, options: options)
        yield @formatter if block_given?
      end

      def call(env)
        @formatter.request(env)
        super
      end

      def on_complete(env)
        @formatter.response(env)
      end

      def on_error(error)
        @formatter.error(error) if @formatter.respond_to?(:error)
      end
    end
