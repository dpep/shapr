class S
  class << self
    def cursor(x = nil)
      x || 1
    end

    def page_size(n = nil)
      n ? (@page_size = n) : (@page_size || 10)
    end
  end
end
