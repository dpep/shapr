class Store < Base
  def save(record)
    items.each { |record| super(record) }
  end
end
