class S
  module Limits
    def page_size(n = nil)
      n ? (@page_size = n) : (@page_size || 10)
    end
  end
end
