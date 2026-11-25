class Store < Base
  def save(record, options = {})
    log(record)
    super
  end
end
