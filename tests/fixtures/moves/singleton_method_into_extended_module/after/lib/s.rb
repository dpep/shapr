require_relative "limits"

class S
  extend Limits

  class << self
    def cursor(x = nil)
      x || 1
    end
  end
end
